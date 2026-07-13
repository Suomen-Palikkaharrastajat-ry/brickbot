use sqlx::SqlitePool;

pub async fn hide_event(
    db: &SqlitePool,
    guild_id: &str,
    brick_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO hidden_events (guild_id, brick_id) VALUES (?, ?) ON CONFLICT DO NOTHING",
    )
    .bind(guild_id)
    .bind(brick_id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn unhide_event(
    db: &SqlitePool,
    guild_id: &str,
    brick_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM hidden_events WHERE guild_id = ? AND brick_id = ?")
        .bind(guild_id)
        .bind(brick_id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn get_hidden_events(
    db: &SqlitePool,
    guild_id: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT brick_id FROM hidden_events WHERE guild_id = ?")
            .bind(guild_id)
            .fetch_all(db)
            .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}
