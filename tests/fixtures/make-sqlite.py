#!/usr/bin/env python3
"""Generate the sample.sqlite fixture (stdlib sqlite3 only).

Run via tests/fixtures/make-sqlite.sh.
"""

import os
import sqlite3

HERE = os.path.dirname(os.path.abspath(__file__))


def main():
    path = os.path.join(HERE, "sample.sqlite")
    if os.path.exists(path):
        os.remove(path)
    conn = sqlite3.connect(path)
    conn.executescript(
        """
        CREATE TABLE sales (
            id INTEGER,
            "amount, usd" REAL,
            region TEXT,
            "note text" TEXT
        );
        CREATE TABLE notes (
            id INTEGER,
            payload BLOB
        );
        CREATE VIEW big_sales AS
            SELECT * FROM sales WHERE "amount, usd" > 15;
        CREATE TABLE "weird ""quoted"" table" (
            id INTEGER,
            label TEXT
        );
        """
    )
    conn.executemany(
        'INSERT INTO sales (id, "amount, usd", region, "note text") VALUES (?, ?, ?, ?)',
        [
            (1, 10.5, "EU", "row 1"),
            (2, 20.25, "US", None),
            (3, 30.0, "APAC", "row 3"),
        ],
    )
    conn.executemany(
        "INSERT INTO notes (id, payload) VALUES (?, ?)",
        [
            (1, b"\x00\x01\x02binary"),
            (2, None),
        ],
    )
    conn.execute('INSERT INTO "weird ""quoted"" table" (id, label) VALUES (1, ?)', ("hi",))
    conn.commit()
    conn.close()

    verify(path)


def verify(path):
    print("=== verify ===")
    conn = sqlite3.connect(path)
    for name, in conn.execute(
        "SELECT name FROM sqlite_master WHERE type IN ('table', 'view') ORDER BY rowid"
    ):
        quoted = '"' + name.replace('"', '""') + '"'
        rows = conn.execute(f"SELECT COUNT(*) FROM {quoted}").fetchone()[0]
        print(f"{name}: {rows} rows")
    conn.close()


if __name__ == "__main__":
    main()
