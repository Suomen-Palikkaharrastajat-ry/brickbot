#!/usr/bin/env bash
# One-off repair after 7ea9d9a ("Announce Atom entries by their alternate link").
#
# Before that fix, Blogger articles were stored under their comment feed's URL
# (https://<blog>/feeds/<n>/comments/default) instead of the post's link. This:
#
#   1. re-points queued digest articles (feed_pending_announcements) at the
#      post's real link, found in feed_items by source and title — the fixed bot
#      stores it there on its first poll, so run this AFTER deploying the fix
#      and letting it poll once, and BEFORE the next digest goes out;
#   2. drops queued articles still carrying a comment-feed link (no real link
#      found, or already queued under it), which would otherwise be posted
#      broken;
#   3. drops the comment-feed rows from feed_items, so /set stops returning
#      them alongside (or instead of) the real article.
#
# Usage: fix-blogger-feed-links.sh [--apply] [path/to/bot.db]
# Without --apply it only reports what it would do. With --apply it first
# writes a backup next to the database. Safe to run while the bot is running.
set -euo pipefail

apply=false
if [[ "${1:-}" == "--apply" ]]; then
    apply=true
    shift
fi
db="${1:-data/bot.db}"

if ! command -v sqlite3 >/dev/null; then
    echo "sqlite3 not found (try: nix shell nixpkgs#sqlite)" >&2
    exit 1
fi
if [[ ! -f "$db" ]]; then
    echo "no database at $db" >&2
    exit 1
fi

# The Blogger comment feed an article was mistakenly identified by.
pattern="'%/feeds/%/comments/default'"
# The real link of a queued article: the same source and title, stored under
# a link that is not a comment feed.
real_link="(SELECT f.id FROM feed_items f
            WHERE f.source_title = feed_pending_announcements.source_title
              AND f.item_title = feed_pending_announcements.item_title
              AND f.id NOT LIKE $pattern
            ORDER BY f.rowid DESC LIMIT 1)"

echo "== Queued digest articles with a comment-feed link"
sqlite3 -cmd ".timeout 5000" -header -column "$db" "
    SELECT channel_id, item_title,
           COALESCE($real_link, '(no real link found: will be dropped)') AS new_link
    FROM feed_pending_announcements
    WHERE item_id LIKE $pattern;"

echo
echo "== feed_items rows with a comment-feed link"
sqlite3 -cmd ".timeout 5000" "$db" "
    SELECT COUNT(*) || ' rows, of which '
        || COALESCE(SUM(EXISTS (SELECT 1 FROM feed_items n
                       WHERE n.source_title = o.source_title
                         AND n.item_title = o.item_title
                         AND n.id NOT LIKE $pattern)), 0)
        || ' have a row with the real link'
    FROM feed_items o WHERE o.id LIKE $pattern;"

if ! $apply; then
    echo
    echo "Dry run. Re-run with --apply to change $db."
    exit 0
fi

backup="$db.before-blogger-fix.$(date +%Y%m%d%H%M%S)"
sqlite3 -cmd ".timeout 5000" "$db" ".backup '$backup'"
echo
echo "Backup written to $backup"

sqlite3 -cmd ".timeout 5000" -bail "$db" "
    BEGIN IMMEDIATE;
    UPDATE OR IGNORE feed_pending_announcements
       SET item_id = $real_link
     WHERE item_id LIKE $pattern AND $real_link IS NOT NULL;
    SELECT 'Re-pointed queued articles: ' || changes();
    DELETE FROM feed_pending_announcements WHERE item_id LIKE $pattern;
    SELECT 'Dropped queued articles:    ' || changes();
    DELETE FROM feed_items WHERE id LIKE $pattern;
    SELECT 'Dropped feed_items rows:    ' || changes();
    COMMIT;"
