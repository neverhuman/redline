use redlinedb::Database;

fn main() -> redlinedb::Result<()> {
    let path = std::env::temp_dir().join("redlinedb-readme.redline");
    let db = Database::create(&path)?;
    let mut conn = db.connect()?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS kv(k INTEGER PRIMARY KEY, v TEXT NOT NULL)",
        (),
    )?;
    conn.execute("INSERT OR REPLACE INTO kv VALUES (1, 'hello')", ())?;

    let value: String = conn.query_row("SELECT v FROM kv WHERE k = 1", ())?;
    println!("{value}");
    Ok(())
}
