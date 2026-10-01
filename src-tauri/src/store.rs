use rusqlite::{params, Connection, OptionalExtension, Result as SqlResult};

use crate::model::{now, Annotation, BookMeta, Bookmark};

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &std::path::Path) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        // WAL keeps reads non-blocking; NORMAL sync is the right trade-off for
        // local reading metadata that is cheap to rebuild.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        Self::migrate(&conn)?;
        Ok(Self { conn })
    }

    fn migrate(conn: &Connection) -> SqlResult<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS books (
                id          TEXT PRIMARY KEY,
                path        TEXT NOT NULL UNIQUE,
                title       TEXT NOT NULL DEFAULT '',
                author      TEXT NOT NULL DEFAULT '',
                language    TEXT NOT NULL DEFAULT '',
                publisher   TEXT NOT NULL DEFAULT '',
                description TEXT NOT NULL DEFAULT '',
                identifier  TEXT NOT NULL DEFAULT '',
                rights      TEXT NOT NULL DEFAULT '',
                cover_href  TEXT,
                file_size   INTEGER NOT NULL DEFAULT 0,
                mtime       INTEGER NOT NULL DEFAULT 0,
                added_at    INTEGER NOT NULL DEFAULT 0,
                last_opened INTEGER,
                progress    REAL NOT NULL DEFAULT 0,
                locator     TEXT NOT NULL DEFAULT ''
            );

            CREATE TABLE IF NOT EXISTS annotations (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                book_id     TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
                href        TEXT NOT NULL,
                selector    TEXT NOT NULL DEFAULT '',
                quote       TEXT NOT NULL DEFAULT '',
                note        TEXT NOT NULL DEFAULT '',
                color       TEXT NOT NULL DEFAULT 'yellow',
                created_at  INTEGER NOT NULL,
                modified_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_annotations_book
                ON annotations(book_id, href);

            CREATE TABLE IF NOT EXISTS bookmarks (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                book_id    TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
                href       TEXT NOT NULL,
                locator    TEXT NOT NULL DEFAULT '',
                label      TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_bookmarks_book
                ON bookmarks(book_id);

            CREATE TABLE IF NOT EXISTS settings (
                k TEXT PRIMARY KEY,
                v TEXT NOT NULL
            );
            "#,
        )
    }

    pub fn list_books(&self) -> SqlResult<Vec<BookMeta>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, title, author, language, publisher, description,
                    identifier, rights, cover_href, file_size, mtime, added_at,
                    last_opened, progress, locator
             FROM books ORDER BY COALESCE(last_opened, added_at) DESC",
        )?;
        let rows = stmt.query_map([], Self::row_to_book)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn get_book(&self, id: &str) -> SqlResult<Option<BookMeta>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, title, author, language, publisher, description,
                    identifier, rights, cover_href, file_size, mtime, added_at,
                    last_opened, progress, locator
             FROM books WHERE id = ?1",
        )?;
        stmt.query_row([id], Self::row_to_book).optional()
    }

    fn row_to_book(row: &rusqlite::Row<'_>) -> SqlResult<BookMeta> {
        Ok(BookMeta {
            id: row.get(0)?,
            path: row.get(1)?,
            title: row.get(2)?,
            author: row.get(3)?,
            language: row.get(4)?,
            publisher: row.get(5)?,
            description: row.get(6)?,
            identifier: row.get(7)?,
            rights: row.get(8)?,
            cover_href: row.get(9)?,
            file_size: row.get(10)?,
            mtime: row.get(11)?,
            added_at: row.get(12)?,
            last_opened: row.get(13)?,
            progress: row.get(14)?,
            locator: row.get(15)?,
            chapters: 0,
        })
    }

    /// Insert or update the row identified by `id`, keeping reading progress.
    pub fn upsert_book(&self, book: &BookMeta) -> SqlResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO books (id, path, title, author, language, publisher,
                               description, identifier, rights, cover_href,
                               file_size, mtime, added_at, last_opened, progress, locator)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                    ?14, ?15, ?16)
            ON CONFLICT(id) DO UPDATE SET
                path        = excluded.path,
                title       = excluded.title,
                author      = excluded.author,
                language    = excluded.language,
                publisher   = excluded.publisher,
                description = excluded.description,
                identifier  = excluded.identifier,
                rights      = excluded.rights,
                cover_href  = excluded.cover_href,
                file_size   = excluded.file_size,
                mtime       = excluded.mtime,
                progress    = excluded.progress,
                locator     = excluded.locator,
                last_opened = COALESCE(excluded.last_opened, books.last_opened)
            "#,
            params![
                book.id,
                book.path,
                book.title,
                book.author,
                book.language,
                book.publisher,
                book.description,
                book.identifier,
                book.rights,
                book.cover_href,
                book.file_size,
                book.mtime,
                book.added_at,
                book.last_opened,
                book.progress,
                book.locator,
            ],
        )?;
        Ok(())
    }

    pub fn set_progress(&self, id: &str, progress: f64, locator: &str) -> SqlResult<()> {
        let ts = now();
        self.conn.execute(
            "UPDATE books SET progress = ?2, locator = ?3, last_opened = ?4 WHERE id = ?1",
            params![id, progress.clamp(0.0, 1.0), locator, ts],
        )?;
        Ok(())
    }

    pub fn touch(&self, id: &str) -> SqlResult<()> {
        self.conn
            .execute("UPDATE books SET last_opened = ?2 WHERE id = ?1", params![id, now()])?;
        Ok(())
    }

    pub fn update_details(
        &self,
        id: &str,
        title: &str,
        author: &str,
        progress: f64,
        locator: &str,
    ) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE books SET title = ?2, author = ?3, progress = ?4, locator = ?5 WHERE id = ?1",
            params![id, title, author, progress.clamp(0.0, 1.0), locator],
        )?;
        Ok(())
    }

    pub fn remove_book(&self, id: &str) -> SqlResult<()> {
        self.conn
            .execute("DELETE FROM annotations WHERE book_id = ?1", [id])?;
        self.conn
            .execute("DELETE FROM bookmarks WHERE book_id = ?1", [id])?;
        self.conn.execute("DELETE FROM books WHERE id = ?1", [id])?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn clear_all(&self) -> SqlResult<()> {
        self.conn.execute_batch(
            "DELETE FROM annotations; DELETE FROM bookmarks; DELETE FROM books;",
        )?;
        Ok(())
    }

    // ---- annotations ----------------------------------------------------

    pub fn add_annotation(
        &self,
        book_id: &str,
        href: &str,
        selector: &str,
        quote: &str,
        note: &str,
        color: &str,
    ) -> SqlResult<Annotation> {
        let ts = now();
        self.conn.execute(
            "INSERT INTO annotations (book_id, href, selector, quote, note, color, created_at, modified_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![book_id, href, selector, quote, note, color, ts],
        )?;
        Ok(Annotation {
            id: self.conn.last_insert_rowid(),
            book_id: book_id.to_string(),
            href: href.to_string(),
            selector: selector.to_string(),
            quote: quote.to_string(),
            note: note.to_string(),
            color: color.to_string(),
            created_at: ts,
            modified_at: ts,
        })
    }

    pub fn update_annotation(
        &self,
        id: i64,
        note: Option<&str>,
        color: Option<&str>,
        quote: Option<&str>,
    ) -> SqlResult<()> {
        let ts = now();
        if let Some(note) = note {
            self.conn.execute(
                "UPDATE annotations SET note = ?2, modified_at = ?3 WHERE id = ?1",
                params![id, note, ts],
            )?;
        }
        if let Some(color) = color {
            self.conn.execute(
                "UPDATE annotations SET color = ?2, modified_at = ?3 WHERE id = ?1",
                params![id, color, ts],
            )?;
        }
        if let Some(quote) = quote {
            self.conn.execute(
                "UPDATE annotations SET quote = ?2, modified_at = ?3 WHERE id = ?1",
                params![id, quote, ts],
            )?;
        }
        Ok(())
    }

    pub fn delete_annotation(&self, id: i64) -> SqlResult<()> {
        self.conn
            .execute("DELETE FROM annotations WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn list_annotations(&self, book_id: &str, href: Option<&str>) -> SqlResult<Vec<Annotation>> {
        let params: Vec<Box<dyn rusqlite::ToSql>> = match href {
            Some(href) => vec![Box::new(book_id.to_string()), Box::new(href.to_string())],
            None => vec![Box::new(book_id.to_string())],
        };
        let sql = match href {
            Some(_) => {
                "SELECT id, book_id, href, selector, quote, note, color, created_at, modified_at
                 FROM annotations WHERE book_id = ?1 AND href = ?2 ORDER BY created_at"
            }
            None => {
                "SELECT id, book_id, href, selector, quote, note, color, created_at, modified_at
                 FROM annotations WHERE book_id = ?1 ORDER BY created_at DESC"
            }
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params.iter().map(|b| b.as_ref())), Self::annotation)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn annotation(row: &rusqlite::Row<'_>) -> SqlResult<Annotation> {
        Ok(Annotation {
            id: row.get(0)?,
            book_id: row.get(1)?,
            href: row.get(2)?,
            selector: row.get(3)?,
            quote: row.get(4)?,
            note: row.get(5)?,
            color: row.get(6)?,
            created_at: row.get(7)?,
            modified_at: row.get(8)?,
        })
    }

    // ---- bookmarks ------------------------------------------------------

    pub fn add_bookmark(
        &self,
        book_id: &str,
        href: &str,
        locator: &str,
        label: &str,
    ) -> SqlResult<Bookmark> {
        let ts = now();
        self.conn.execute(
            "INSERT INTO bookmarks (book_id, href, locator, label, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![book_id, href, locator, label, ts],
        )?;
        Ok(Bookmark {
            id: self.conn.last_insert_rowid(),
            book_id: book_id.to_string(),
            href: href.to_string(),
            locator: locator.to_string(),
            label: label.to_string(),
            created_at: ts,
        })
    }

    pub fn delete_bookmark(&self, id: i64) -> SqlResult<()> {
        self.conn
            .execute("DELETE FROM bookmarks WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn list_bookmarks(&self, book_id: &str) -> SqlResult<Vec<Bookmark>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, book_id, href, locator, label, created_at
             FROM bookmarks WHERE book_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([book_id], |row| {
            Ok(Bookmark {
                id: row.get(0)?,
                book_id: row.get(1)?,
                href: row.get(2)?,
                locator: row.get(3)?,
                label: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    // ---- settings -------------------------------------------------------

    pub fn get_settings(&self) -> SqlResult<serde_json::Value> {
        let mut stmt = self.conn.prepare("SELECT k, v FROM settings")?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
        let mut map = serde_json::Map::new();
        for row in rows {
            let (k, v) = row?;
            let parsed: serde_json::Value =
                serde_json::from_str(&v).unwrap_or(serde_json::Value::String(v));
            map.insert(k, parsed);
        }
        Ok(serde_json::Value::Object(map))
    }

    pub fn set_setting(&self, key: &str, value: &serde_json::Value) -> SqlResult<()> {
        self.conn.execute(
            "INSERT INTO settings (k, v) VALUES (?1, ?2)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            params![key, value.to_string()],
        )?;
        Ok(())
    }
}