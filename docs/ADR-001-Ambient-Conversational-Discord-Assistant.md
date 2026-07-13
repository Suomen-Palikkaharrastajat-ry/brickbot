# ADR-001: Ambient Conversational Assistant for Discord

## Status
Accepted

## Date
2026-06-25 (Updated: 2026-07-15)

## Context

Traditional Discord bots rely on slash commands (e.g., `/help`, `/search`). This model requires users to know the bot exists, know its commands, and remember the exact syntax. In active communities, most users never discover advanced bot capabilities.

We want a bot that:
- Observes conversation passively.
- Detects opportunities to help automatically.
- Does not interrupt unnecessarily.
- Requires explicit user consent before taking up channel space.
- Minimizes channel noise by defaulting to private, ephemeral interactions.
- Scales to busy channels.

---

## Decision

Adopt an Ambient Conversational Assistant architecture using:

1. Passive Topic Detection (Keyword/Regex matching)
2. Cooldown & Confidence Safeguards
3. Minimal Public Suggestion (Single Message with Buttons)
4. Ephemeral Follow-Up Responses (Discord Message Components)
5. Strict Privacy Rules (No public leakage of private states)

---

## Architecture

```text
User Conversation
        |
        v
+------------------+
| Topic Detection  |
+------------------+
        |
        v
+------------------+
| Cooldown Checks  |
+------------------+
        |
        +---- On Cooldown (Ignore)
        |
        +---- Valid (Offer Suggestion)
                 |
                 v
      Minimal Public Button
                 |
                 v
           User Clicks
                 |
                 v
      Ephemeral Response
                 |
                 v
       Private Workflow Flow
```

---

## Topic Detection & Safeguards

The bot continuously processes channel messages. Detection (e.g., matching a LEGO set number) alone MUST NOT trigger a large public payload.

The assistant MUST avoid becoming a distraction. Safeguards include:
- **Global & Topic Cooldowns**: Enforce strict cooldowns per channel (e.g., max one suggestion per topic per channel every 30 minutes, or 12 hours for the exact same item).
- **Opt-In/Out Channels**: The assistant only acts in explicitly configured ambient channels.
- **User Ignore Lists**: Users can opt-out of ambient tracking entirely.

---

## Minimal Public Interaction

The assistant MUST require explicit user interaction before expanding.
When the bot detects a topic (e.g., a LEGO set), it posts the smallest acceptable public component message.

Example:
**User**: "Does anyone have thoughts on set 42083?"
**Bot** (Minimal Public Message):
`[ View 42083 Bugatti Chiron ] [ Related Articles ]`

This prevents unsolicited information dumps in public channels.

---

## Privacy & Ephemeral Responses

Interactions SHOULD be ephemeral whenever possible. All user-initiated command flows or ambient interactions MUST result in an ephemeral response (visible only to the invoking user).

Advantages:
- Reduces channel noise to near-zero.
- Personalizes interactions without bothering other members.
- Supports sensitive information or moderation contexts.

**Rule**: The public message must not contain private workflow state, Zulip moderation content, or user-specific rationale. Once the user clicks the public button, the entire subsequent flow becomes ephemeral.

---

## Consequences

### Positive
- High discoverability without requiring users to memorize slash commands.
- Natural, non-intrusive interactions.
- Lower channel clutter due to ephemeral follow-ups.
- Stronger engagement on topics.

### Negative
- Requires maintaining state management and strict cooldown tracking (SQLite).
- Risk of over-intervention if cooldowns are misconfigured.
- Ephemeral interactions expire (Discord interaction tokens are valid for 15 minutes), requiring users to restart workflows if they time out.

---

## Final Recommendation

For modern Discord communities:

**DO:**
- Passive topic detection with strict cooldowns.
- Minimal public Discord buttons for explicit consent.
- Ephemeral responses for all follow-up data.

**DO NOT:**
- Depend entirely on slash commands for discovery.
- Trigger large response embeds automatically in public.
- Interrupt conversations without user consent.
