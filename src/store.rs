//! SQLite-backed history + pinboards, with de-duplication and retention.

use crate::model::*;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::path::{Path, PathBuf};

pub struct Store {
    conn: Connection,
    images_dir: PathBuf,
}

/// Query parameters for `Store::list`.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub query: String,
    pub kind: Option<ItemKind>,
    pub board: Option<i64>,
    pub app: Option<String>,
    pub limit: usize,
}

const COLS: &str = "id, kind, text, image_file, title, source_app, source_bundle, \
                    created_at, pinboard_id, bytes, width, height";

fn row_to_item(r: &Row) -> rusqlite::Result<ClipItem> {
    let kind: String = r.get(1)?;
    Ok(ClipItem {
        id: r.get(0)?,
        kind: ItemKind::parse(&kind).unwrap_or(ItemKind::Text),
        text: r.get(2)?,
        rtf: None,
        html: None,
        image_file: r.get(3)?,
        title: r.get(4)?,
        source_app: r.get(5)?,
        source_bundle: r.get(6)?,
        created_at: r.get(7)?,
        pinboard_id: r.get(8)?,
        bytes: r.get::<_, i64>(9)? as u64,
        width: r.get::<_, i64>(10)? as u32,
        height: r.get::<_, i64>(11)? as u32,
    })
}

impl Store {
    /// Open (or create) the store inside `dir`.
    pub fn open(dir: &Path) -> Result<Store> {
        std::fs::create_dir_all(dir.join("images"))?;
        let conn = Connection::open(dir.join("history.sqlite"))?;
        Self::init(conn, dir.join("images"))
    }

    /// In-memory database; images still go to `images_dir`. Used by tests.
    pub fn open_memory(images_dir: &Path) -> Result<Store> {
        std::fs::create_dir_all(images_dir)?;
        Self::init(Connection::open_in_memory()?, images_dir.to_path_buf())
    }

    fn init(conn: Connection, images_dir: PathBuf) -> Result<Store> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS items(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                text TEXT NOT NULL DEFAULT '',
                rtf BLOB,
                html BLOB,
                image_file TEXT,
                title TEXT,
                source_app TEXT NOT NULL DEFAULT '',
                source_bundle TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL,
                pinboard_id INTEGER,
                hash TEXT NOT NULL UNIQUE,
                bytes INTEGER NOT NULL DEFAULT 0,
                width INTEGER NOT NULL DEFAULT 0,
                height INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS idx_items_created ON items(created_at DESC);
             CREATE TABLE IF NOT EXISTS boards(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                color TEXT NOT NULL
             );",
        )?;
        // Upgrade databases created by versions that stored only RTF.
        let has_html = {
            let mut stmt = conn.prepare("PRAGMA table_info(items)")?;
            let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
            columns
                .collect::<rusqlite::Result<Vec<_>>>()?
                .iter()
                .any(|name| name == "html")
        };
        if !has_html {
            conn.execute("ALTER TABLE items ADD COLUMN html BLOB", [])?;
        }
        Ok(Store { conn, images_dir })
    }

    pub fn image_path(&self, file: &str) -> PathBuf {
        self.images_dir.join(file)
    }

    /// Insert a captured clip. Returns `(id, inserted)`; when identical content
    /// already exists it is moved to the front instead (`inserted == false`).
    /// Returns `None` for empty clips.
    pub fn insert(&mut self, clip: &NewClip, now_ms: i64) -> Result<Option<(i64, bool)>> {
        let kind = if clip.image_png.is_some() {
            ItemKind::Image
        } else if (clip.rtf.is_some() || clip.html.is_some()) && !clip.text.trim().is_empty() {
            ItemKind::RichText
        } else {
            clip.resolved_kind()
        };
        if kind != ItemKind::Image && clip.text.trim().is_empty() {
            return Ok(None);
        }
        let hash = clip.content_hash(kind);

        if let Some(id) = self
            .conn
            .query_row("SELECT id FROM items WHERE hash=?1", [&hash], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
        {
            self.conn.execute(
                "UPDATE items SET created_at=?1, source_app=?2, source_bundle=?3,
                                  title=COALESCE(?4,title) WHERE id=?5",
                params![now_ms, clip.source_app, clip.source_bundle, clip.title, id],
            )?;
            return Ok(Some((id, false)));
        }

        let text = if kind == ItemKind::Color {
            parse_color_str(&clip.text)
                .map(|(_, h)| h)
                .unwrap_or_else(|| clip.text.clone())
        } else {
            clip.text.clone()
        };

        let (image_file, bytes) = match &clip.image_png {
            Some(png) => {
                let name = format!("{hash}.png");
                std::fs::write(self.images_dir.join(&name), png)?;
                (Some(name), png.len() as u64)
            }
            None => (None, text.len() as u64),
        };
        let title = clip.title.clone().or_else(|| match kind {
            ItemKind::File => text
                .lines()
                .next()
                .and_then(|p| Path::new(p).file_name())
                .map(|n| n.to_string_lossy().into_owned()),
            ItemKind::Code => extract_suggested_title(&text, kind),
            _ => None,
        });
        self.conn.execute(
            "INSERT INTO items(kind,text,rtf,html,image_file,title,source_app,source_bundle,
                               created_at,hash,bytes,width,height)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                kind.as_str(),
                text,
                clip.rtf,
                clip.html,
                image_file,
                title,
                clip.source_app,
                clip.source_bundle,
                now_ms,
                hash,
                bytes as i64,
                clip.width as i64,
                clip.height as i64
            ],
        )?;
        Ok(Some((self.conn.last_insert_rowid(), true)))
    }

    /// Full item including RTF payload.
    pub fn get(&self, id: i64) -> Result<Option<ClipItem>> {
        let item = self
            .conn
            .query_row(
                &format!("SELECT {COLS} FROM items WHERE id=?1"),
                [id],
                row_to_item,
            )
            .optional()?;
        let Some(mut item) = item else {
            return Ok(None);
        };
        item.rtf = self
            .conn
            .query_row("SELECT rtf FROM items WHERE id=?1", [id], |r| r.get(0))?;
        item.html = self
            .conn
            .query_row("SELECT html FROM items WHERE id=?1", [id], |r| r.get(0))?;
        Ok(Some(item))
    }

    /// Newest first. The text query is a case-insensitive substring match over
    /// text, title and source app.
    pub fn list(&self, f: &Filter) -> Result<Vec<ClipItem>> {
        let mut sql = format!("SELECT {COLS} FROM items WHERE 1=1");
        let mut args: Vec<rusqlite::types::Value> = vec![];
        if let Some(k) = f.kind {
            sql += " AND kind=?";
            args.push(k.as_str().to_string().into());
        }
        if let Some(b) = f.board {
            sql += " AND pinboard_id=?";
            args.push(b.into());
        }
        if let Some(a) = &f.app {
            sql += " AND source_app=?";
            args.push(a.clone().into());
        }
        sql += " ORDER BY created_at DESC, id DESC";
        let q = f.query.trim().to_lowercase();
        let limit = if f.limit == 0 { usize::MAX } else { f.limit };
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args), row_to_item)?;
        let mut out = Vec::new();
        for row in rows {
            let item = row?;
            if !q.is_empty() {
                let hay = format!(
                    "{}\n{}\n{}",
                    item.text,
                    item.title.as_deref().unwrap_or(""),
                    item.source_app
                )
                .to_lowercase();
                if !hay.contains(&q) {
                    continue;
                }
            }
            out.push(item);
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Distinct source apps, for the app filter.
    pub fn apps(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT source_app FROM items WHERE source_app<>'' ORDER BY source_app COLLATE NOCASE",
        )?;
        let v = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    pub fn delete(&mut self, id: i64) -> Result<()> {
        let file: Option<String> = self
            .conn
            .query_row("SELECT image_file FROM items WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?
            .flatten();
        self.conn.execute("DELETE FROM items WHERE id=?1", [id])?;
        if let Some(f) = file {
            let _ = std::fs::remove_file(self.images_dir.join(f));
        }
        Ok(())
    }

    /// Move an item to the front of the history.
    pub fn touch(&mut self, id: i64, now_ms: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE items SET created_at=?1 WHERE id=?2",
            params![now_ms, id],
        )?;
        Ok(())
    }

    pub fn rename(&mut self, id: i64, title: &str) -> Result<()> {
        let t = title.trim();
        self.conn.execute(
            "UPDATE items SET title=?1 WHERE id=?2",
            params![if t.is_empty() { None } else { Some(t) }, id],
        )?;
        Ok(())
    }

    /// Replace the plain text of a text-like item (in-place editing).
    /// Merges into an existing identical item if the edit produces a duplicate.
    pub fn edit_text(&mut self, id: i64, text: &str) -> Result<()> {
        let Some(item) = self.get(id)? else {
            return Ok(());
        };
        if matches!(item.kind, ItemKind::Image | ItemKind::File) {
            return Ok(());
        }
        let clip = NewClip {
            text: text.to_string(),
            ..Default::default()
        };
        let kind = clip.resolved_kind();
        let hash = clip.content_hash(kind);
        let clash: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM items WHERE hash=?1 AND id<>?2",
                params![hash, id],
                |r| r.get(0),
            )
            .optional()?;
        if clash.is_some() {
            return Ok(()); // an identical item already exists; keep the original
        }
        self.conn.execute(
            "UPDATE items SET text=?1, rtf=NULL, kind=?2, hash=?3, bytes=?4 WHERE id=?5",
            params![text, kind.as_str(), hash, text.len() as i64, id],
        )?;
        Ok(())
    }

    pub fn set_pin(&mut self, id: i64, board: Option<i64>) -> Result<()> {
        self.conn.execute(
            "UPDATE items SET pinboard_id=?1 WHERE id=?2",
            params![board, id],
        )?;
        Ok(())
    }

    pub fn create_board(&mut self, name: &str, color: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO boards(name,color) VALUES(?1,?2)",
            params![name.trim(), color],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn boards(&self) -> Result<Vec<Pinboard>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id,name,color FROM boards ORDER BY id")?;
        let v = stmt
            .query_map([], |r| {
                Ok(Pinboard {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    color: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    pub fn rename_board(&mut self, id: i64, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE boards SET name=?1 WHERE id=?2",
            params![name.trim(), id],
        )?;
        Ok(())
    }

    /// Deleting a board unpins its items (they return to normal history).
    pub fn delete_board(&mut self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE items SET pinboard_id=NULL WHERE pinboard_id=?1",
            [id],
        )?;
        self.conn.execute("DELETE FROM boards WHERE id=?1", [id])?;
        Ok(())
    }

    /// Apply retention. Pinned items never expire. Returns rows deleted.
    pub fn purge(&mut self, retention_days: u32, max_items: u32, now_ms: i64) -> Result<usize> {
        let mut doomed: Vec<(i64, Option<String>)> = vec![];
        let mut collect = |sql: &str, args: &[&dyn rusqlite::ToSql]| -> Result<()> {
            let mut stmt = self.conn.prepare(sql)?;
            let rows = stmt.query_map(args, |r| Ok((r.get(0)?, r.get(1)?)))?;
            for r in rows {
                doomed.push(r?);
            }
            Ok(())
        };
        if retention_days > 0 {
            let cutoff = now_ms - retention_days as i64 * 86_400_000;
            collect(
                "SELECT id, image_file FROM items WHERE pinboard_id IS NULL AND created_at<?1",
                &[&cutoff],
            )?;
        }
        if max_items > 0 {
            collect(
                "SELECT id, image_file FROM items WHERE pinboard_id IS NULL
                 ORDER BY created_at DESC, id DESC LIMIT -1 OFFSET ?1",
                &[&(max_items as i64)],
            )?;
        }
        doomed.sort_by_key(|d| d.0);
        doomed.dedup_by_key(|d| d.0);
        for (id, file) in &doomed {
            self.conn.execute("DELETE FROM items WHERE id=?1", [id])?;
            if let Some(f) = file {
                let _ = std::fs::remove_file(self.images_dir.join(f));
            }
        }
        Ok(doomed.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tag: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("rp-store-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Store::open_memory(&dir).unwrap()
    }

    fn text(s: &str) -> NewClip {
        NewClip {
            text: s.into(),
            source_app: "Test".into(),
            ..Default::default()
        }
    }

    #[test]
    fn dedupe_moves_to_front() {
        let mut s = store("dedupe");
        let (a, ins) = s.insert(&text("one"), 1000).unwrap().unwrap();
        assert!(ins);
        s.insert(&text("two"), 2000).unwrap().unwrap();
        let (a2, ins2) = s.insert(&text("one"), 3000).unwrap().unwrap();
        assert_eq!(a, a2);
        assert!(!ins2);
        let items = s.list(&Filter::default()).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text, "one");
        assert_eq!(items[0].created_at, 3000);
    }

    #[test]
    fn empty_is_skipped_and_kinds_classified() {
        let mut s = store("kinds");
        assert!(s.insert(&text("   "), 1).unwrap().is_none());
        s.insert(&text("https://rust-lang.org"), 2).unwrap();
        s.insert(&text("#FF9400"), 3).unwrap();
        let items = s.list(&Filter::default()).unwrap();
        assert_eq!(items[0].kind, ItemKind::Color);
        assert_eq!(items[1].kind, ItemKind::Link);
    }

    #[test]
    fn rich_text_roundtrip_and_distinct_from_plain() {
        let mut s = store("rtf");
        let rich = NewClip {
            text: "hi".into(),
            rtf: Some(b"{\\rtf1 hi}".to_vec()),
            ..Default::default()
        };
        let (id, _) = s.insert(&rich, 1).unwrap().unwrap();
        let (id2, ins) = s.insert(&text("hi"), 2).unwrap().unwrap();
        assert!(ins);
        assert_ne!(id, id2);
        let got = s.get(id).unwrap().unwrap();
        assert_eq!(got.kind, ItemKind::RichText);
        assert_eq!(got.rtf.as_deref(), Some(&b"{\\rtf1 hi}"[..]));
    }

    #[test]
    fn image_stored_and_removed() {
        let mut s = store("img");
        let clip = NewClip {
            image_png: Some(vec![1, 2, 3, 4]),
            width: 2,
            height: 2,
            ..Default::default()
        };
        let (id, _) = s.insert(&clip, 1).unwrap().unwrap();
        let item = s.get(id).unwrap().unwrap();
        let path = s.image_path(item.image_file.as_ref().unwrap());
        assert!(path.exists());
        assert_eq!(item.footer(), "2 × 2 · 4 B");
        s.delete(id).unwrap();
        assert!(!path.exists());
        assert!(s.get(id).unwrap().is_none());
    }

    #[test]
    fn search_and_filters() {
        let mut s = store("search");
        s.insert(&text("Hello World"), 1).unwrap();
        s.insert(&text("https://example.com"), 2).unwrap();
        let f = Filter {
            query: "hello".into(),
            ..Default::default()
        };
        assert_eq!(s.list(&f).unwrap().len(), 1);
        let f = Filter {
            query: "test".into(),
            ..Default::default()
        }; // matches source app
        assert_eq!(s.list(&f).unwrap().len(), 2);
        let f = Filter {
            kind: Some(ItemKind::Link),
            ..Default::default()
        };
        assert_eq!(s.list(&f).unwrap().len(), 1);
        let f = Filter {
            limit: 1,
            ..Default::default()
        };
        assert_eq!(s.list(&f).unwrap().len(), 1);
    }

    #[test]
    fn pinboards_and_retention() {
        let mut s = store("pins");
        let b = s.create_board("Snippets", "#FF9400").unwrap();
        let (old_pinned, _) = s.insert(&text("keep"), 0).unwrap().unwrap();
        s.insert(&text("old"), 0).unwrap();
        s.insert(&text("new"), 10 * 86_400_000).unwrap();
        s.set_pin(old_pinned, Some(b)).unwrap();

        let f = Filter {
            board: Some(b),
            ..Default::default()
        };
        assert_eq!(s.list(&f).unwrap().len(), 1);

        let removed = s.purge(5, 0, 10 * 86_400_000).unwrap();
        assert_eq!(removed, 1);
        let texts: Vec<_> = s
            .list(&Filter::default())
            .unwrap()
            .into_iter()
            .map(|i| i.text)
            .collect();
        assert_eq!(texts, vec!["new", "keep"]);

        // max_items counts only unpinned; pinned survive.
        s.insert(&text("a"), 11 * 86_400_000).unwrap();
        s.insert(&text("b"), 12 * 86_400_000).unwrap();
        s.purge(0, 1, 12 * 86_400_000).unwrap();
        let texts: Vec<_> = s
            .list(&Filter::default())
            .unwrap()
            .into_iter()
            .map(|i| i.text)
            .collect();
        assert_eq!(texts, vec!["b", "keep"]);

        // Deleting a board unpins.
        s.delete_board(b).unwrap();
        assert!(s.boards().unwrap().is_empty());
        assert!(s.get(old_pinned).unwrap().unwrap().pinboard_id.is_none());
    }

    #[test]
    fn edit_and_rename() {
        let mut s = store("edit");
        let (id, _) = s.insert(&text("abc"), 1).unwrap().unwrap();
        s.edit_text(id, "https://x.org").unwrap();
        let it = s.get(id).unwrap().unwrap();
        assert_eq!(it.text, "https://x.org");
        assert_eq!(it.kind, ItemKind::Link);
        s.rename(id, " My link ").unwrap();
        assert_eq!(
            s.get(id).unwrap().unwrap().title.as_deref(),
            Some("My link")
        );
        s.rename(id, "").unwrap();
        assert!(s.get(id).unwrap().unwrap().title.is_none());
    }
}
