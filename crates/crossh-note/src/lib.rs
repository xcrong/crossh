//! 纯逻辑笔记存储：SQLite + FTS5，零 gpui 依赖

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
/// 单条笔记的字节上限。SQLite TEXT 本身能存到 1 GB，这里只挡病态输入；
/// 超限一律**拒绝**而非截断——静默砍掉用户写了一半的内容比报错糟糕得多。
/// 早期版本用 8 KB + 截断，导致长笔记被砍一半且 `is_dirty` 永久为真（存进去的
/// 是截断版，编辑器里是全文，两者永远不等）。
pub const MAX_CONTENT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Note {
    pub id: i64,
    pub content: String,
    pub pinned: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// 校验待写入的内容：trim 后非空、且原文不超过 [`MAX_CONTENT_BYTES`]。
/// 返回错误码字符串供 UI 直接展示（`empty_content` / `content_too_large`）。
///
/// 只校验、不改写：早前这里返回 trim 后的文本再落库，Markdown 笔记首行的缩进
/// （缩进代码块）和结尾空行会被吃掉——用户看到自己写的文本，读回来却是另一份。
/// 空内容判定仍需 trim，因为纯空白笔记在列表里等于一条看不见的空行。
fn validate_content(content: &str) -> Result<(), String> {
    if content.trim().is_empty() {
        return Err("empty_content".to_string());
    }
    if content.len() > MAX_CONTENT_BYTES {
        return Err("content_too_large".to_string());
    }
    Ok(())
}

/// 备份路径：`note.db.corrupt.<秒>.<纳秒>`，撞名时再加 `-<n>`。
/// 旧实现只有秒级时间戳，而 Unix 的 `rename` 会静默覆盖同名目标——同一秒内
/// 两次备份，第二次（往往是个刚建的空库）就会盖掉上一份真正的旧库。
fn corrupt_backup_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "note.db".to_string());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let stamp = format!("{}.{:09}", now.as_secs(), now.subsec_nanos());
    let mut seq = 0u64;
    loop {
        let candidate = if seq == 0 {
            path.with_file_name(format!("{}.corrupt.{}", file_name, stamp))
        } else {
            path.with_file_name(format!("{}.corrupt.{}-{}", file_name, stamp, seq))
        };
        if !candidate.exists() {
            return candidate;
        }
        seq += 1;
    }
}

/// `<db>-wal` / `<db>-shm` 的路径。SQLite 的侧车文件就是主库路径加后缀。
fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// 把侧车搬到备份名：优先 `rename`，失败退化为 `copy` + `remove_file`。
///
/// `rename` 要求源和目标是同一设备、且两个目录都可写（跨设备 EXDEV、目录只读都会失败），
/// 而“拷到备份名”只需要读源文件 + 写目标目录。侧车里的页本身就是数据，搬不动就等于丢。
fn move_sidecar(from: &Path, to: &Path) -> Result<(), String> {
    if let Err(rename_error) = std::fs::rename(from, to) {
        // copy 成功即算“已备份”；原文件删不掉时留在原地，由调用方 warn 出来，
        // 但错误串必须带上两段真实原因，不能只说“失败了”。
        return std::fs::copy(from, to)
            .and_then(|_| std::fs::remove_file(from))
            .map_err(|copy_error| {
                format!(
                    "rename failed: {}; copy fallback failed: {}",
                    rename_error, copy_error
                )
            });
    }
    Ok(())
}

/// 把不可用的库搬去备份名：主库必搬，`-wal`/`-shm` 能搬就搬。
/// 主库的 rename 失败必须上报：把不存在的路径当成备份返回给上层，等于谎报“已备份”。
/// 侧车只是 best-effort——主库这时已经在备份名下了，若因为侧车失败就把整个打开流程
/// 报成错误，用户会同时失去“旧库可见”和“应用可用”两件事。
///
/// 但侧车不能随便留在原名下：只有**头损坏**的 WAL 才会被 SQLite 判为无效自行忽略；
/// 结构完整、校验和正确的旧 WAL（主库被外部改坏而 WAL 完好，正是洗库的典型场景）
/// 会被当成这个库自己的 WAL 参与恢复，而新空库紧接着就建在同一个路径上。
/// 所以侧车搬不动时退化为 copy + remove（见 [`move_sidecar`]），两者都做不到才留在原地
/// 并 warn：此时数据在备份名下还有一份，但残页是否被新库认领没有保证。
fn backup_unusable_db(path: &Path) -> Result<PathBuf, String> {
    let backup = corrupt_backup_path(path);
    std::fs::rename(path, &backup).map_err(|e| format!("backup {:?} failed: {}", path, e))?;
    for suffix in ["-wal", "-shm"] {
        let from = sidecar_path(path, suffix);
        if !from.exists() {
            continue;
        }
        let to = sidecar_path(&backup, suffix);
        if let Err(e) = move_sidecar(&from, &to) {
            log::warn!(
                "backup {:?} failed (main db already backed up, continuing): {}",
                from,
                e
            );
        }
    }
    Ok(backup)
}

/// [`open_usable_sqlite`] 的失败原因。只有“文件内容不是 SQLite 库”才允许洗库重建，
/// 其余错误必须原样上抛且一个字节都不动。
enum OpenFailure {
    /// 文件确实不是可用 SQLite（`NotADatabase` / `DatabaseCorrupt`）
    Corrupt(String),
    /// 无法证明文件损坏：锁竞争、打不开、io 错误、只读、内存不足……
    NotDamaged(String),
}

/// 判定表：`NotADatabase` / `DatabaseCorrupt` → 损坏，其余一律不算。
/// `SQLITE_BUSY`（另一实例持写事务超过 busy_timeout）、`SQLITE_CANTOPEN`
/// （路径是目录、父目录不可写）、`SQLITE_IOERR`、`SQLITE_READONLY`、`SQLITE_NOMEM`
/// 都会穿过同一条探测路径；把它们也判成损坏，等于替用户把库改名搬走再换成空库。
fn classify_open_error(error: rusqlite::Error) -> OpenFailure {
    let corrupt = matches!(
        &error,
        rusqlite::Error::SqliteFailure(e, _)
            if matches!(
                e.code,
                rusqlite::ffi::ErrorCode::NotADatabase | rusqlite::ffi::ErrorCode::DatabaseCorrupt
            )
    );
    let message = error.to_string();
    if corrupt {
        OpenFailure::Corrupt(message)
    } else {
        OpenFailure::NotDamaged(message)
    }
}

/// 打开文件并做窄探测：只确认它能被当成 SQLite 读。这是“库损坏”的**唯一**判据，
/// 且只有 [`OpenFailure::Corrupt`] 才算数——[`ensure_schema`] 的失败面还包括
/// `database is locked` / `disk full` / `readonly` / io error（多窗口、磁盘满都会命中），
/// 把那些也当成损坏去洗库，等于用户笔记被无端清空且没有任何报错。
fn open_usable_sqlite(path: &Path) -> Result<Connection, OpenFailure> {
    let conn = Connection::open(path).map_err(classify_open_error)?;
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .map(|_| ())
    .map_err(classify_open_error)?;
    Ok(conn)
}

pub fn note_db_path() -> PathBuf {
    if let Some(path) = test_path_override() {
        return path;
    }
    let base = dirs::config_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("crossh").join("note.db")
}

// 仅测试使用：线程隔离的路径覆盖，隔离并行单测
thread_local! {
    static TEST_PATH: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn test_path_override() -> Option<PathBuf> {
    TEST_PATH.with(|c| c.borrow().clone())
}

#[cfg(test)]
pub fn set_test_path(path: PathBuf) {
    TEST_PATH.with(|c| *c.borrow_mut() = Some(path));
}

#[cfg(test)]
pub fn clear_test_path() {
    TEST_PATH.with(|c| *c.borrow_mut() = None);
}

/// 结构版本。改动 `notes` / `notes_fts` / FTS trigger 定义就 +1：
/// 只有版本落后时才会重建 FTS 索引并回填、重跑 trigger，避免每次启动都把整库重写一遍。
/// trigger 的存在性只按**名字**判断（见 [`schema_is_current`]），所以改 trigger 定义
/// 必须靠版本号推进才会落到老库上，否则 `user_version` 已相等的库永远留着旧定义。
const SCHEMA_VERSION: i64 = 3;

const FTS_TRIGGERS: [&str; 3] = ["notes_ai", "notes_ad", "notes_au"];

/// FTS 的三个同步 trigger。
/// `notes_au` 只跟随 `content`：软删 / 恢复 / 置顶都不改正文，不带 `OF content`
/// 时每次置顶都要把单条最多 1 MiB 的正文重新 untokenize + tokenize 一遍。
const CREATE_FTS_TRIGGERS: &str = r#"
DROP TRIGGER IF EXISTS notes_ai;
DROP TRIGGER IF EXISTS notes_ad;
DROP TRIGGER IF EXISTS notes_au;
CREATE TRIGGER notes_ai AFTER INSERT ON notes BEGIN
    INSERT INTO notes_fts(rowid, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER notes_ad AFTER DELETE ON notes BEGIN
    INSERT INTO notes_fts(notes_fts, rowid, content) VALUES ('delete', old.id, old.content);
END;
CREATE TRIGGER notes_au AFTER UPDATE OF content ON notes BEGIN
    INSERT INTO notes_fts(notes_fts, rowid, content) VALUES ('delete', old.id, old.content);
    INSERT INTO notes_fts(rowid, content) VALUES (new.id, new.content);
END;
"#;

/// 探测 `notes` 的列。读 PRAGMA 失败必须上抛：吞成“空列集合”会让调用方去补一个
/// 本来就存在的列，拿到 `duplicate column name`，再被误判成库损坏。
fn note_columns(conn: &Connection) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("SELECT name FROM pragma_table_info('notes')")
        .map_err(|e| e.to_string())?;
    let iter = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for c in iter {
        out.push(c.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// SQLite 列名不区分大小写：库里若是 `Pinned`，用 `==` 会判成缺列，接着
/// `ALTER TABLE ... ADD COLUMN pinned` 撞 `duplicate column name`（补列是硬错），整库打不开。
fn has_column(cols: &[String], name: &str) -> bool {
    cols.iter().any(|c| c.eq_ignore_ascii_case(name))
}

/// 库对象是否存在。trigger 与 FTS 的存在性判断都走 `sqlite_master`，风格统一。
fn object_exists(conn: &Connection, kind: &str, name: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type=?1 AND name=?2",
        params![kind, name],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .map_err(|e| e.to_string())
}

/// 结构是否已经是当前版本：`notes` 新列齐全、FTS 表在、三个 trigger 在。
/// 只有全中才允许走零 DDL 的打开路径。
///
/// 故意**容忍**残留的 `tags` 列：`ALTER TABLE ... DROP COLUMN tags` 失败只 warn
/// （让一个 drop 不掉的库永远打不开，比多留个不参与任何读写的列严重得多），
/// 而版本号照写。若这里仍把 tags 判为 not current，这类库每次启动都会
/// `DROP TRIGGER` + `CREATE TRIGGER` + 写 `user_version`，正是多窗口抢写锁的来源。
fn schema_is_current(conn: &Connection) -> Result<bool, String> {
    let cols = note_columns(conn)?;
    if !has_column(&cols, "pinned") || !has_column(&cols, "deleted_at") {
        return Ok(false);
    }
    for trigger in FTS_TRIGGERS {
        if !object_exists(conn, "trigger", trigger)? {
            return Ok(false);
        }
    }
    object_exists(conn, "table", "notes_fts")
}

/// 建表 / 迁移列 / 建索引。收 `&Connection` 纯函数式：重试与备份在
/// [`NoteStore::open`] 里做，不再把 `MutexGuard` 穿进内层。
fn ensure_schema(conn: &Connection) -> Result<(), String> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    // 比程序新的库：连 journal_mode 都不写，直接上抛。
    // 重建条件用的是 `version < SCHEMA_VERSION`，但末尾无条件写版本号，
    // 不挡住更高版本就会把未知的新库静默降级标成旧版（元数据改坏、结构却没降），
    // 之后新旧程序会各自在一份混合状态的库上再迁移一遍。
    if version > SCHEMA_VERSION {
        return Err(format!(
            "note.db schema version {} is newer than this app supports ({})",
            version, SCHEMA_VERSION
        ));
    }
    // journal_mode 落在库头、foreign_keys 是连接级，这两个每次打开都要设。
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
        .map_err(|e| e.to_string())?;

    // 快路径：结构已是最新就一条 DDL 都不发。DROP + CREATE trigger 是写事务，
    // 多窗口同时启动时会互相抢写锁，是 `database is locked` 的主要来源。
    if version == SCHEMA_VERSION && schema_is_current(conn)? {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS notes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            content TEXT NOT NULL,
            pinned INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            deleted_at INTEGER
        );
        "#,
    )
    .map_err(|e| e.to_string())?;

    let cols = note_columns(conn)?;
    // 老库去 tags 列：无历史包袱，直接删列，不留双写。
    // 必须先删 trigger：它们引用了 new.tags / old.tags，DROP COLUMN 会因此失败。
    if has_column(&cols, "tags") {
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS notes_ai; DROP TRIGGER IF EXISTS notes_ad; DROP TRIGGER IF EXISTS notes_au;",
        )
        .map_err(|e| e.to_string())?;
        // 删列失败不致命：tags 不参与任何读写，留着也不影响数据（不吞掉日志即可）。
        // 让 drop 失败变成硬错就等于“一个 drop 不掉的库永远打不开”，比多留一列严重得多。
        // 配套：[`schema_is_current`] 容忍残留 tags，这类库不会因此每次启动都重跑 DDL。
        //
        // 前提写全：所有写路径都按列名列举 insert/update，残留 tags 必须有**默认值**
        // 才不会挡住写入。本程序历史版本建的 tags 均为 `NOT NULL DEFAULT ''`，所以
        // warn 放行是安全的。若碰上 `NOT NULL` 且无默认值的外来库，这个 warn 会变成“库能开、
        // 每次 create/update 都 `NOT NULL constraint failed`”，而版本号已写 3、drop 永不再重试。
        if let Err(e) = conn.execute("ALTER TABLE notes DROP COLUMN tags", []) {
            log::warn!("remove tags column failed: {}", e);
        }
    }
    // 补列必须硬错：先前用 warn 放过后照样写版本号，此后 `list` / `get` / `search_like`
    // 每条查询都是 `no such column`，而且版本已是新版永远不再重试——库就此报废。
    if !has_column(&cols, "pinned") {
        conn.execute(
            "ALTER TABLE notes ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|e| format!("migrate pinned column failed: {}", e))?;
    }
    // 软删列：NULL = 未删除，所以补列后既有行天然全部“未删”。
    if !has_column(&cols, "deleted_at") {
        conn.execute("ALTER TABLE notes ADD COLUMN deleted_at INTEGER", [])
            .map_err(|e| format!("migrate deleted_at column failed: {}", e))?;
    }

    // FTS 只在版本落后或表缺失时重建（DROP + 全量回填会重写整个索引，不该每次启动都做）
    let fts_exists = object_exists(conn, "table", "notes_fts")?;
    if version < SCHEMA_VERSION || !fts_exists {
        conn.execute_batch(
            r#"
            DROP TABLE IF EXISTS notes_fts;
            CREATE VIRTUAL TABLE notes_fts USING fts5(content, content='notes', content_rowid='id', tokenize='unicode61');
            INSERT INTO notes_fts(rowid, content) SELECT id, content FROM notes;
            "#,
        )
        .map_err(|e| e.to_string())?;
    }

    // trigger 定义随时可重建（DDL 不涉数据），且必须与当前 FTS 列一致。
    // 注意得在建好 notes_fts 之后：SQLite 建 trigger 时会校验引用对象存在。
    conn.execute_batch(CREATE_FTS_TRIGGERS)
        .map_err(|e| e.to_string())?;

    // 版本号最后写，写之前复核列：任何一步静默失败都不许把库标成新版，
    // 否则后续查询撞 `no such column`，且版本已满、永远不再重试。
    let cols = note_columns(conn)?;
    if !has_column(&cols, "pinned") || !has_column(&cols, "deleted_at") {
        return Err("notes table still misses pinned/deleted_at after migration".to_string());
    }
    conn.execute_batch(&format!("PRAGMA user_version={};", SCHEMA_VERSION))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 把查询按空白切成检索词；纯空白返回空列表（= 不检索）。
fn search_terms(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_string).collect()
}

/// LIKE 通配符按字面匹配（反斜杠先转义，否则 `ESCAPE '\\'` 会被查询里的 `\` 破坏）。
fn escape_like(term: &str) -> String {
    term.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// 一条正文在本次查询上的命中证据：命中次数与首次命中位置（字节下标）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MatchEvidence {
    hits: usize,
    first_index: Option<usize>,
}

/// 只用正文能算出的证据。大小写折叠限定 ASCII，与 SQLite `LIKE` 和 FTS tokenizer
/// 的范围一致；用 `to_lowercase()` 做全 Unicode 折叠会出现「证据说有命中、SQL 说没有」的
/// 隐形偏差，排序结果就会取决于哪条路径先把笔记送进候选集。
fn match_evidence(content: &str, terms: &[String]) -> MatchEvidence {
    let haystack = content.to_ascii_lowercase();
    let mut evidence = MatchEvidence::default();
    for term in terms {
        let needle = term.to_ascii_lowercase();
        if needle.is_empty() {
            continue;
        }
        for (index, _) in haystack.match_indices(&needle) {
            evidence.hits += 1;
            evidence.first_index = Some(match evidence.first_index {
                Some(first) => first.min(index),
                None => index,
            });
        }
    }
    evidence
}

/// 统一排序：置顶 → 命中次数 → 首次命中位置 → 最近更新 → id。
///
/// 只用两条检索路径都能算出的证据：FTS 的 `bm25` 只在 FTS 命中的行上有分值，
/// 拿它排序会让中文命中永久排在后面。
fn rank_notes(notes: Vec<Note>, terms: &[String]) -> Vec<Note> {
    let mut ranked = notes
        .into_iter()
        .map(|note| (match_evidence(&note.content, terms), note))
        .collect::<Vec<_>>();
    ranked.sort_by(|(left_evidence, left), (right_evidence, right)| {
        right
            .pinned
            .cmp(&left.pinned)
            .then_with(|| right_evidence.hits.cmp(&left_evidence.hits))
            .then_with(|| {
                left_evidence
                    .first_index
                    .unwrap_or(usize::MAX)
                    .cmp(&right_evidence.first_index.unwrap_or(usize::MAX))
            })
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| right.id.cmp(&left.id))
    });
    ranked.into_iter().map(|(_, note)| note).collect()
}

pub struct NoteStore {
    pub path: PathBuf,
    /// 本次打开时若把损坏的旧库备份走了，这里是被备份文件的实际路径
    /// （`note.db.corrupt.<ts>`）。上层据此告诉用户“笔记去了哪”，而不是只看到
    /// 一个空列表。正常打开为 `None`。
    pub recovered_from: Option<PathBuf>,
    conn: Mutex<Connection>,
}

impl NoteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        // 只有“文件根本不是可用 SQLite”才备份重建。`database is locked` / `disk full` /
        // `readonly` / `cantopen` / io error 一律原样上抛：文件不动，用户数据不动，
        // 下次启动还能自愈（洗库会把这些问题变成“笔记凭空消失”）。
        let mut recovered_from = None;
        let conn = match open_usable_sqlite(&path) {
            Ok(conn) => conn,
            Err(OpenFailure::Corrupt(probe_error)) => {
                log::warn!("note.db is not a usable sqlite database: {}", probe_error);
                // 先备份（主库必搬、WAL/SHM 能搬就搬），再在同一路径建新库
                let backup = backup_unusable_db(&path)?;
                // 重试失败的错误串必须带上真实备份路径：用户唯一的旧数据就在那里，
                // 只说“打开失败”等于让人无从找回。措辞也不能一律叫 corrupt——
                // 重试碰到锁 / io 错误时文件并没有坏，谎报原因会把用户引向错误处置。
                recovered_from = Some(backup.clone());
                open_usable_sqlite(&path).map_err(|e| match e {
                    OpenFailure::Corrupt(m) => {
                        format!("note.db corrupt after retry (backup: {:?}): {}", backup, m)
                    }
                    OpenFailure::NotDamaged(m) => format!(
                        "note.db open retry failed after backup (backup: {:?}): {}",
                        backup, m
                    ),
                })?
            }
            Err(OpenFailure::NotDamaged(error)) => {
                return Err(format!("note.db open failed: {}", error));
            }
        };
        // 两个窗口同时首启时，`ensure_schema` 里的 `PRAGMA journal_mode=WAL` 要拿排他锁；
        // 默认 busy_timeout 为 0，碰锁立刻返回 SQLITE_BUSY，被归为 NotDamaged，
        // 用户看到的是“笔记库打开失败”。5 秒覆盖另一个进程跑完迁移的时间。
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| format!("note.db busy_timeout failed: {}", e))?;
        // 迁移失败只上报，不动文件。洗过库的话要把备份路径一起报出来，
        // 否则用户只知道打不开，不知道旧数据被搬去了哪里。
        if let Err(e) = ensure_schema(&conn) {
            return Err(match &recovered_from {
                Some(backup) => {
                    format!("note.db ensure_schema failed (backup: {:?}): {}", backup, e)
                }
                None => format!("note.db ensure_schema failed: {}", e),
            });
        }
        Ok(Self {
            path,
            recovered_from,
            conn: Mutex::new(conn),
        })
    }

    pub fn open_default() -> Result<Self, String> {
        Self::open(note_db_path())
    }

    pub fn list(&self) -> Result<Vec<Note>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT id, content, pinned, created_at, updated_at FROM notes WHERE deleted_at IS NULL ORDER BY pinned DESC, updated_at DESC, id DESC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Note {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    pinned: row.get::<_, i64>(2)? != 0,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    /// 检索：FTS 候选与正文子串候选的并集，再按 [`rank_notes`] 统一排序。
    ///
    /// ponytail: 子串路径是随「笔记数 × 正文大小」线性增长的扫描，而且每条查询都会跑
    /// （纯 ASCII 查询也一样，否则 `wor` 命中 `world` 这类子串匹配会漏）。个人笔记库规模下
    /// 够用；变大时的升级路径是加一列影子 bigram 索引（`SCHEMA_VERSION` +1 并重建），
    /// 而不是给这里加缓存。
    pub fn search(&self, query: &str) -> Result<Vec<Note>, String> {
        let terms = search_terms(query);
        if terms.is_empty() {
            return self.list();
        }
        let mut notes = match self.search_fts(&terms) {
            Ok(found) => found,
            Err(error) => {
                // 索引不可用不该让用户看到「查询失败」：子串路径仍能给出结果。
                log::warn!("note fts query failed, using substring scan: {error}");
                Vec::new()
            }
        };
        let mut seen: HashSet<i64> = notes.iter().map(|note| note.id).collect();
        for note in self.search_like(&terms)? {
            if seen.insert(note.id) {
                notes.push(note);
            }
        }
        Ok(rank_notes(notes, &terms))
    }

    fn search_fts(&self, terms: &[String]) -> Result<Vec<Note>, String> {
        // 每个词包成双引号字面量，避免 FTS5 把 `*`、`NEAR(` 等当语法。
        let match_expr = terms
            .iter()
            .map(|term| format!("\"{}\"", term.replace('\"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let conn = self.conn.lock();
        let sql = "SELECT n.id, n.content, n.pinned, n.created_at, n.updated_at FROM notes n JOIN notes_fts f ON n.id = f.rowid WHERE notes_fts MATCH ? AND n.deleted_at IS NULL ORDER BY n.pinned DESC, n.updated_at DESC, n.id DESC";
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![match_expr], |row| {
                Ok(Note {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    pinned: row.get::<_, i64>(2)? != 0,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    /// 子串路径：任一词出现在正文即命中（`OR`）。中文在 FTS 上零命中，靠这条路径。
    fn search_like(&self, terms: &[String]) -> Result<Vec<Note>, String> {
        let conn = self.conn.lock();
        let clause = vec!["content LIKE ? ESCAPE '\\'"; terms.len()].join(" OR ");
        let sql = format!(
            "SELECT id, content, pinned, created_at, updated_at FROM notes WHERE deleted_at IS NULL AND ({clause}) ORDER BY pinned DESC, updated_at DESC, id DESC"
        );
        let patterns = terms
            .iter()
            .map(|term| format!("%{}%", escape_like(term)))
            .collect::<Vec<_>>();
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params_from_iter(patterns), |row| {
                Ok(Note {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    pinned: row.get::<_, i64>(2)? != 0,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    pub fn create(&self, content: &str) -> Result<Note, String> {
        validate_content(content)?;
        let now = now_ts();
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO notes (content, pinned, created_at, updated_at) VALUES (?1, 0, ?2, ?3)",
            params![content, now, now],
        )
        .map_err(|e| e.to_string())?;
        let id = conn.last_insert_rowid();
        Ok(Note {
            id,
            content: content.to_string(),
            pinned: false,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn update(&self, id: i64, content: &str) -> Result<Note, String> {
        validate_content(content)?;
        let now = now_ts();
        let conn = self.conn.lock();
        // 已软删的行对用户不可见，写路径也不许碰：否则看不见的文本被改写，
        // 之后 restore 出来的是被偷偷改过的内容。
        let changed = conn
            .execute(
                "UPDATE notes SET content=?1, updated_at=?2 WHERE id=?3 AND deleted_at IS NULL",
                params![content, now, id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err("not_found".to_string());
        }
        let mut stmt = conn
            .prepare("SELECT id, content, pinned, created_at, updated_at FROM notes WHERE id=?1")
            .map_err(|e| e.to_string())?;
        let note = stmt
            .query_row(params![id], |row| {
                Ok(Note {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    pinned: row.get::<_, i64>(2)? != 0,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?;
        Ok(note)
    }

    pub fn set_pinned(&self, id: i64, pinned: bool) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = now_ts();
        let changed = conn
            .execute(
                "UPDATE notes SET pinned=?1, updated_at=?2 WHERE id=?3 AND deleted_at IS NULL",
                params![if pinned { 1 } else { 0 }, now, id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err("not_found".to_string());
        }
        Ok(())
    }

    /// 软删：只打 `deleted_at` 时间戳。硬删没有任何中间态，⌘D 两次就永久丢数据，
    /// 而用户按错一次的成本远高于库里多留一行的成本。
    /// 目标不存在或已经删过都返回 `not_found`，与 [`Self::update`] 的错误语义一致。
    pub fn delete(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock();
        let changed = conn
            .execute(
                "UPDATE notes SET deleted_at=?1 WHERE id=?2 AND deleted_at IS NULL",
                params![now_ts(), id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err("not_found".to_string());
        }
        Ok(())
    }

    /// 撤销软删。目标不存在或本来就没删 → `not_found`。
    pub fn restore(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock();
        let changed = conn
            .execute(
                "UPDATE notes SET deleted_at=NULL WHERE id=?1 AND deleted_at IS NOT NULL",
                params![id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err("not_found".to_string());
        }
        Ok(())
    }

    /// 物理清掉软删时间不晚于 `now - older_than_secs` 的行，返回干净的行数。
    /// 软删行不能永远长下去，但保留期交给调用方定义（本层不内置定期任务）。
    /// 比较用 `<=`：`purge_deleted(0)` 的语义是“现在就清干净”，边界那一行也得算进去。
    pub fn purge_deleted(&self, older_than_secs: i64) -> Result<usize, String> {
        let conn = self.conn.lock();
        let cutoff = now_ts() - older_than_secs;
        conn.execute(
            "DELETE FROM notes WHERE deleted_at IS NOT NULL AND deleted_at <= ?1",
            params![cutoff],
        )
        .map_err(|e| e.to_string())
    }

    /// 彻底清空（含已软删的行）：调用方明确要求“全部没了”时没有保留期可言。
    pub fn clear_all(&self) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM notes", [])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get(&self, id: i64) -> Result<Option<Note>, String> {
        // 故意不过滤 `deleted_at`：恢复与数据校验都需要读到已删行。
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT id, content, pinned, created_at, updated_at FROM notes WHERE id=?1")
            .map_err(|e| e.to_string())?;
        let res = stmt
            .query_row(params![id], |row| {
                Ok(Note {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    pinned: row.get::<_, i64>(2)? != 0,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            })
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    use super::*;
    use tempfile::TempDir;

    fn tmp_store() -> (NoteStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.db");
        let store = NoteStore::open(&path).unwrap();
        (store, dir)
    }

    /// 目录里 `.corrupt.*` 备份文件的名字。断言“没洗库”就靠它是空的。
    fn corrupt_backups(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".corrupt."))
            .collect()
    }

    fn schema_version_of(path: &Path) -> i64 {
        Connection::open(path)
            .unwrap()
            .query_row("PRAGMA schema_version", [], |r| r.get(0))
            .unwrap()
    }

    fn user_version_of(path: &Path) -> i64 {
        Connection::open(path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn spec_20260827_note__create_and_list() {
        let (store, _dir) = tmp_store();
        let note = store.create("hello").unwrap();
        assert_eq!(note.content, "hello");
        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, note.id);
    }

    #[test]
    fn spec_20260827_note__empty_content_rejected() {
        let (store, _dir) = tmp_store();
        assert_eq!(store.create("   ").unwrap_err(), "empty_content");
        // 纯空白（含换行/制表符）同样等于一条看不见的空笔记
        assert_eq!(store.create("   \n\t").unwrap_err(), "empty_content");
        assert_eq!(store.list().unwrap().len(), 0);
    }

    #[test]
    fn spec_20260827_note__content_stored_verbatim() {
        // 首行 4 空格缩进（缩进代码块）和结尾空行都是笔记内容的一部分。
        // 旧实现落库前 trim，读回来的是被修剪过的另一份文本。
        let (store, _dir) = tmp_store();
        let source = "    let x = 1;\n正文\n\n";
        let note = store.create(source).unwrap();
        assert_eq!(note.content, source);
        assert_eq!(store.get(note.id).unwrap().unwrap().content, source);
        let update_source = "\t缩进\n\n";
        let updated = store.update(note.id, update_source).unwrap();
        assert_eq!(updated.content, update_source);
        assert_eq!(store.get(note.id).unwrap().unwrap().content, update_source);
    }

    #[test]
    fn spec_20260827_note__max_bytes_rejected_not_truncated() {
        // 回归：旧实现按 8 KB 静默截断，用户写的长笔记被砍掉一半且无从察觉。
        // 现在超限一律报错，内容不落库。
        let (store, _dir) = tmp_store();
        let big = "a".repeat(MAX_CONTENT_BYTES + 100);
        assert_eq!(store.create(&big).unwrap_err(), "content_too_large");
        assert_eq!(store.list().unwrap().len(), 0);
        // 恰好等于上限应放行（边界不误伤）。
        let exact = "a".repeat(MAX_CONTENT_BYTES);
        let note = store.create(&exact).unwrap();
        assert_eq!(note.content.len(), MAX_CONTENT_BYTES);
        // update 同样拒绝，且不改动原有内容。
        assert_eq!(
            store.update(note.id, &big).unwrap_err(),
            "content_too_large"
        );
        assert_eq!(
            store.get(note.id).unwrap().unwrap().content.len(),
            MAX_CONTENT_BYTES
        );
    }

    #[test]
    fn spec_20260827_note__update_changes_updated_at() {
        let (store, _dir) = tmp_store();
        let note = store.create("a").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        let updated = store.update(note.id, "b").unwrap();
        assert_eq!(updated.content, "b");
        assert!(updated.updated_at >= note.updated_at);
    }

    #[test]
    fn spec_20260827_note__pin_orders_first() {
        let (store, _dir) = tmp_store();
        let n1 = store.create("a").unwrap();
        let n2 = store.create("b").unwrap();
        store.set_pinned(n1.id, true).unwrap();
        let list = store.list().unwrap();
        assert_eq!(list[0].id, n1.id);
        assert_eq!(list[1].id, n2.id);
    }

    #[test]
    fn spec_20260827_note__search_content() {
        let (store, _dir) = tmp_store();
        store.create("hello world").unwrap();
        store.create("other").unwrap();
        let res = store.search("hello").unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].content, "hello world");
    }

    #[test]
    fn spec_20260827_note__search_like_escape() {
        let (store, _dir) = tmp_store();
        store.create("a%b").unwrap();
        store.create("axb").unwrap();
        // 搜索含 % 的内容按字面匹配：不能把 % 当通配符
        let res = store.search("a%b").unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].content, "a%b");
    }

    #[test]
    fn spec_20260827_note__soft_delete_and_restore() {
        let (store, _dir) = tmp_store();
        let n = store.create("hello world").unwrap();
        store.delete(n.id).unwrap();
        // 删除后对用户不可见：列表 / FTS / LIKE 三条路径都得过滤
        assert!(store.list().unwrap().is_empty());
        assert!(store.search_fts(&search_terms("hello")).unwrap().is_empty());
        assert!(
            store
                .search_like(&search_terms("hello"))
                .unwrap()
                .is_empty()
        );
        assert!(store.search("hello").unwrap().is_empty());
        // get 不过滤：恢复与校验需要读到已删行
        assert_eq!(store.get(n.id).unwrap().unwrap().content, "hello world");
        // 重复删除 / 恢复一条未删除的行都是 not_found
        assert_eq!(store.delete(n.id).unwrap_err(), "not_found");
        store.restore(n.id).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        assert_eq!(store.search("hello").unwrap().len(), 1);
        assert_eq!(store.restore(n.id).unwrap_err(), "not_found");
        assert_eq!(store.delete(9999).unwrap_err(), "not_found");
    }

    #[test]
    fn spec_20260827_note__purge_deleted_removes_only_aged_rows() {
        let (store, _dir) = tmp_store();
        let old = store.create("old").unwrap();
        store.delete(old.id).unwrap();
        // 刚删的行不能被保留期 1 小时的清理带走
        assert_eq!(store.purge_deleted(3600).unwrap(), 0);
        // 把删除时间往前拨 2 小时，模拟已超保留期
        store
            .conn
            .lock()
            .execute(
                "UPDATE notes SET deleted_at = deleted_at - 7200 WHERE id=?1",
                params![old.id],
            )
            .unwrap();
        assert_eq!(store.purge_deleted(3600).unwrap(), 1);
        assert!(store.get(old.id).unwrap().is_none());
    }

    #[test]
    fn spec_20260827_note__clear_all() {
        let (store, _dir) = tmp_store();
        store.create("a").unwrap();
        let deleted = store.create("b").unwrap();
        store.delete(deleted.id).unwrap();
        store.clear_all().unwrap();
        // “彻底清空”包含已软删的行，否则清完之后磁盘上还有用户以为删掉的文本
        assert_eq!(store.list().unwrap().len(), 0);
        assert!(store.get(deleted.id).unwrap().is_none());
    }

    #[test]
    fn spec_20260827_note__hash_tag_search_via_content() {
        let (store, _dir) = tmp_store();
        store.create("hello #work idea").unwrap();
        store.create("other").unwrap();
        let res = store.search("#work").unwrap();
        assert_eq!(res.len(), 1);
    }

    #[test]
    fn spec_20261107_note_search__cjk_query_hits() {
        // 中文在 FTS 上零命中，靠子串路径，但不该被描述成「回退」：两条路径是并集。
        let (store, _dir) = tmp_store();
        store.create("今天天气不错，适合散步").unwrap();
        store.create("other english note").unwrap();
        let two = store.search("天气").unwrap();
        assert_eq!(two.len(), 1);
        assert!(two[0].content.contains("天气"));
        // 单字也要能命中：
        let one = store.search("气").unwrap();
        assert_eq!(one.len(), 1);
    }

    #[test]
    fn spec_20261107_note_search__mixed_query_returns_union() {
        let (store, _dir) = tmp_store();
        let english = store.create("hello world").unwrap();
        let chinese = store.create("今天天气不错，适合散步").unwrap();
        let res = store.search("hello 天气").unwrap();
        assert_eq!(res.len(), 2);
        let ids: Vec<i64> = res.iter().map(|note| note.id).collect();
        assert!(ids.contains(&english.id));
        assert!(ids.contains(&chinese.id));
        // 并集不能把同一条笔记放两次
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), ids.len());
    }

    #[test]
    fn spec_20261107_note_search__blank_query_matches_list() {
        let (store, _dir) = tmp_store();
        store.create("alpha").unwrap();
        store.create("beta").unwrap();
        let listed = store.list().unwrap();
        let searched = store.search("   ").unwrap();
        let listed_ids: Vec<i64> = listed.iter().map(|note| note.id).collect();
        let searched_ids: Vec<i64> = searched.iter().map(|note| note.id).collect();
        assert_eq!(searched_ids, listed_ids);
    }

    #[test]
    fn spec_20261107_note_search__no_match_is_empty() {
        let (store, _dir) = tmp_store();
        store.create("hello world").unwrap();
        assert!(store.search("zzz-无匹配").unwrap().is_empty());
    }

    #[test]
    fn spec_20261107_note_search__ascii_case_insensitive() {
        let (store, _dir) = tmp_store();
        store.create("hello world").unwrap();
        assert_eq!(store.search("HELLO").unwrap().len(), 1);
    }

    #[test]
    fn spec_20261107_note_search__soft_deleted_note_is_not_returned() {
        let (store, _dir) = tmp_store();
        let deleted = store.create("hello 已删").unwrap();
        let kept = store.create("hello 保留").unwrap();
        store.delete(deleted.id).unwrap();
        let res = store.search("hello").unwrap();
        let ids: Vec<i64> = res.iter().map(|note| note.id).collect();
        assert_eq!(ids, vec![kept.id]);
    }

    #[test]
    fn spec_20261107_note_search__pinned_outranks_more_hits() {
        let (store, _dir) = tmp_store();
        let noisy = store.create("work work work").unwrap();
        let pinned = store.create("work").unwrap();
        store.set_pinned(pinned.id, true).unwrap();
        let res = store.search("work").unwrap();
        let ids: Vec<i64> = res.iter().map(|note| note.id).collect();
        assert_eq!(ids, vec![pinned.id, noisy.id]);
    }

    #[test]
    fn spec_20261107_note_search__hits_outrank_newer_updates() {
        let (store, _dir) = tmp_store();
        let many_hits = store.create("work work work").unwrap();
        // 等到下一秒，让后创建的笔记 updated_at 严格更新
        std::thread::sleep(std::time::Duration::from_secs(1));
        let newer = store.create("work").unwrap();
        assert!(newer.updated_at > many_hits.updated_at);
        let res = store.search("work").unwrap();
        let ids: Vec<i64> = res.iter().map(|note| note.id).collect();
        assert_eq!(ids, vec![many_hits.id, newer.id]);
    }

    #[test]
    fn spec_20261107_note_search__fts_syntax_chars_do_not_error() {
        let (store, _dir) = tmp_store();
        store.create("work item").unwrap();
        // 未配对引号 / NEAR( / 单个引号都不应让查询变成 Err
        assert!(store.search("\"work").is_ok());
        assert!(store.search("NEAR( work").is_ok());
        assert!(store.search("\"").is_ok());
    }

    #[test]
    fn spec_20261107_note_search__fts_unavailable_falls_back_to_substring() {
        let (store, _dir) = tmp_store();
        store.create("hello world").unwrap();
        // 索引表没了是真实可能的损坏形态：此时仍要靠子串路径出结果，而不是报错
        store
            .conn
            .lock()
            .execute_batch("DROP TABLE notes_fts")
            .unwrap();
        let res = store.search("hello").unwrap();
        assert_eq!(res.len(), 1);
    }

    #[test]
    fn match_evidence_counts_hits_and_first_index() {
        let terms = search_terms("work 天气");
        let evidence = match_evidence("xx work yy work 天气", &terms);
        assert_eq!(evidence.hits, 3);
        assert_eq!(evidence.first_index, Some(3));
        assert_eq!(match_evidence("nothing", &terms).hits, 0);
        assert_eq!(match_evidence("nothing", &terms).first_index, None);
        // 空词不参与统计
        assert_eq!(match_evidence("aaa", &search_terms("   ")).hits, 0);
    }

    #[test]
    fn match_evidence_folds_ascii_case_only() {
        let terms = search_terms("HELLO");
        assert_eq!(match_evidence("say hello", &terms).hits, 1);
        // 非 ASCII 不折叠：与 SQLite LIKE / FTS tokenizer 的范围保持一致
        let umlaut = search_terms("Ä");
        assert_eq!(match_evidence("Ä", &umlaut).hits, 1);
        assert_eq!(match_evidence("ä", &umlaut).hits, 0);
    }

    #[test]
    fn rank_notes_orders_by_pinned_hits_position_updated_and_id() {
        let terms = search_terms("work");
        let notes = vec![
            Note {
                id: 1,
                content: "work".to_string(),
                pinned: false,
                created_at: 0,
                updated_at: 10,
            },
            Note {
                id: 2,
                content: "work work".to_string(),
                pinned: false,
                created_at: 0,
                updated_at: 5,
            },
            Note {
                id: 3,
                content: "work".to_string(),
                pinned: true,
                created_at: 0,
                updated_at: 0,
            },
            Note {
                id: 4,
                content: "xx work".to_string(),
                pinned: false,
                created_at: 0,
                updated_at: 99,
            },
            Note {
                id: 5,
                content: "a work work".to_string(),
                pinned: false,
                created_at: 0,
                updated_at: 50,
            },
        ];
        let ranked = rank_notes(notes, &terms);
        // 置顶 → 命中次数 → 首次位置（同为 2 次时靠前者先） → 最近更新 → id
        let ids: Vec<i64> = ranked.iter().map(|note| note.id).collect();
        assert_eq!(ids, vec![3, 2, 5, 1, 4]);
    }

    #[test]
    fn rank_notes_is_deterministic_for_equal_evidence() {
        let terms = search_terms("work");
        let notes = vec![
            Note {
                id: 7,
                content: "work".to_string(),
                pinned: false,
                created_at: 0,
                updated_at: 1,
            },
            Note {
                id: 9,
                content: "work".to_string(),
                pinned: false,
                created_at: 0,
                updated_at: 1,
            },
        ];
        let first: Vec<i64> = rank_notes(notes.clone(), &terms)
            .iter()
            .map(|note| note.id)
            .collect();
        let second: Vec<i64> = rank_notes(notes, &terms)
            .iter()
            .map(|note| note.id)
            .collect();
        assert_eq!(first, second);
        assert_eq!(first, vec![9, 7]);
    }

    #[test]
    fn spec_20260903_note__schema_has_no_tags() {
        // 建表不再含 tags：notes 列无 tags，notes_fts 定义不含 tags
        let (store, _dir) = tmp_store();
        let conn = store.conn.lock();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('notes')")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|c| c.unwrap())
            .collect();
        assert!(!cols.iter().any(|c| c == "tags"));
        let fts_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='notes_fts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!fts_sql.contains("tags"));
    }

    #[test]
    fn spec_20260903_note__legacy_db_with_tags_opens() {
        // 老库（带 tags 列 / 带 tags 的 FTS）能正常打开，数据不丢
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE notes (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    content TEXT NOT NULL,
                    tags TEXT NOT NULL DEFAULT '',
                    pinned INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE VIRTUAL TABLE notes_fts USING fts5(content, tags, content='notes', content_rowid='id', tokenize='unicode61');
                CREATE TRIGGER notes_ai AFTER INSERT ON notes BEGIN
                    INSERT INTO notes_fts(rowid, content, tags) VALUES (new.id, new.content, new.tags);
                END;
                CREATE TRIGGER notes_ad AFTER DELETE ON notes BEGIN
                    INSERT INTO notes_fts(notes_fts, rowid, content, tags) VALUES ('delete', old.id, old.content, old.tags);
                END;
                CREATE TRIGGER notes_au AFTER UPDATE ON notes BEGIN
                    INSERT INTO notes_fts(notes_fts, rowid, content, tags) VALUES ('delete', old.id, old.content, old.tags);
                    INSERT INTO notes_fts(rowid, content, tags) VALUES (new.id, new.content, new.tags);
                END;
                INSERT INTO notes (content, tags, pinned, created_at, updated_at) VALUES ('legacy hello', '', 0, 1, 1);
                INSERT INTO notes (content, tags, pinned, created_at, updated_at) VALUES ('旧数据中文测试', '', 0, 1, 1);
                "#,
            )
            .unwrap();
        }
        let store = NoteStore::open(&path).unwrap();
        assert_eq!(store.list().unwrap().len(), 2);
        // tags 列已去掉，FTS 定义不含 tags
        let conn = store.conn.lock();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('notes')")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|c| c.unwrap())
            .collect();
        assert!(!cols.iter().any(|c| c == "tags"));
        let fts_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='notes_fts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!fts_sql.contains("tags"));
        drop(conn);
        // 老数据仍可搜到（英文走 FTS，中文走 LIKE 回退），新写入正常
        assert_eq!(store.search("legacy").unwrap().len(), 1);
        assert_eq!(store.search("中文").unwrap().len(), 1);
        store.create("new note").unwrap();
        assert_eq!(store.list().unwrap().len(), 3);
    }

    #[test]
    fn spec_20260827_note__v1_db_gains_soft_delete_without_data_loss() {
        // 上一版（user_version=1，含 pinned、无 deleted_at）必须原地升级：
        // 既有笔记全部视为“未删除”，且能继续软删 / 恢复。
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                r#"
                PRAGMA user_version=1;
                CREATE TABLE notes (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    content TEXT NOT NULL,
                    pinned INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE VIRTUAL TABLE notes_fts USING fts5(content, content='notes', content_rowid='id', tokenize='unicode61');
                CREATE TRIGGER notes_ai AFTER INSERT ON notes BEGIN
                    INSERT INTO notes_fts(rowid, content) VALUES (new.id, new.content);
                END;
                INSERT INTO notes (content, pinned, created_at, updated_at) VALUES ('v1 hello', 1, 1, 1);
                INSERT INTO notes (content, pinned, created_at, updated_at) VALUES ('v1 中文', 0, 1, 1);
                INSERT INTO notes_fts(rowid, content) SELECT id, content FROM notes;
                "#,
            )
            .unwrap();
        }
        let store = NoteStore::open(&path).unwrap();
        assert!(store.recovered_from.is_none());
        assert_eq!(store.list().unwrap().len(), 2);
        let ver: i64 = store
            .conn
            .lock()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ver, SCHEMA_VERSION);
        let id = store.list().unwrap()[0].id;
        store.delete(id).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        store.restore(id).unwrap();
        assert_eq!(store.list().unwrap().len(), 2);
        // 老数据可搜（走 FTS），新写入正常
        assert_eq!(store.search("hello").unwrap().len(), 1);
        store.create("new note").unwrap();
        assert_eq!(store.list().unwrap().len(), 3);
    }

    #[test]
    fn spec_20260827_note__versioned_db_does_not_rebuild_fts_on_reopen() {
        // 已到 `user_version == SCHEMA_VERSION` 的库再打开不该重建索引：先手动搞掉一条索引项，
        // 若重开时仍全量回填，它会被重新搜到——那说明每次启动都在重写整个索引。
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.db");
        {
            let store = NoteStore::open(&path).unwrap();
            let note = store.create("hello world").unwrap();
            store
                .conn
                .lock()
                .execute(
                    "INSERT INTO notes_fts(notes_fts, rowid, content) VALUES ('delete', ?1, ?2)",
                    params![note.id, note.content.as_str()],
                )
                .unwrap();
            assert!(store.search_fts(&search_terms("hello")).unwrap().is_empty());
        }
        let store = NoteStore::open(&path).unwrap();
        assert!(store.search_fts(&search_terms("hello")).unwrap().is_empty());
        // 数据本身仍在，只是索引没被无条件重建
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn spec_20260827_note__corrupt_db_reports_backup_path() {
        // 损坏库会被改名备份，但用户只看到“没有笔记”。备份路径必须回传给上层，
        // 否则数据等于凭空消失，也无法引导用户去找回。
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.db");
        std::fs::write(&path, b"this is not a sqlite database").unwrap();
        let store = NoteStore::open(&path).unwrap();
        let backup = store
            .recovered_from
            .clone()
            .expect("corrupt db should report backup path");
        assert!(backup.exists());
        assert_eq!(
            std::fs::read(&backup).unwrap(),
            b"this is not a sqlite database"
        );
        // 新库可用，后续打开不再报告备份
        store.create("fresh").unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        drop(store);
        let reopened = NoteStore::open(&path).unwrap();
        assert!(reopened.recovered_from.is_none());
        assert_eq!(reopened.list().unwrap().len(), 1);
    }

    #[test]
    fn spec_20261106_note__migration_failure_does_not_wipe_library() {
        // 打开成功、但迁移必然失败：`notes` 是 view，`ALTER TABLE ... ADD COLUMN`
        // 直接报 "Cannot add a column to a view"。这是结构/环境问题，不是文件损坏。
        // 旧实现把它当损坏：rename 走用户的库、建个空库，笔记从此消失。
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA user_version=1; CREATE VIEW notes AS SELECT 1 AS id;")
                .unwrap();
        }
        let err = match NoteStore::open(&path) {
            Ok(_) => panic!("migration failure must not be treated as a corrupt db"),
            Err(e) => e,
        };
        assert!(err.contains("pinned"), "unexpected error: {}", err);
        // 原文件还在原路径，一个 .corrupt.* 都没产生
        assert!(path.exists());
        let backups = corrupt_backups(dir.path());
        assert!(
            backups.is_empty(),
            "non-corrupt error wiped the db: {:?}",
            backups
        );
        // 版本号没被写上去：下次启动还会重跑迁移，仍有自愈机会
        assert_eq!(user_version_of(&path), 1);
    }

    #[test]
    fn spec_20261106_note__corrupt_backup_moves_wal_sidecars() {
        // WAL 库的备份必须把 -wal / -shm 一起搬走：只搬主库会丢掉未 checkpoint 的页，
        // 残留的 -wal 还会被新建的空库认领（旧页重放，或新库直接打不开）。
        // 这里直接测备份函数：SQLite 探测时自己会把“看起来无效”的 -wal 删掉，
        // 真正要守住的是“搬的时候一个都不能漏”。
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.db");
        std::fs::write(&path, b"not a database").unwrap();
        let wal = sidecar_path(&path, "-wal");
        let shm = sidecar_path(&path, "-shm");
        std::fs::write(&wal, b"stale wal pages").unwrap();
        std::fs::write(&shm, b"stale shm").unwrap();

        let backup = backup_unusable_db(&path).unwrap();
        // 原路径一个残留都没留
        assert!(!path.exists());
        assert!(!wal.exists());
        assert!(!shm.exists());
        // 三个文件按同一个备份名挨个落地，旧页（数据）没丢
        assert_eq!(std::fs::read(&backup).unwrap(), b"not a database");
        assert_eq!(
            std::fs::read(sidecar_path(&backup, "-wal")).unwrap(),
            b"stale wal pages"
        );
        assert_eq!(
            std::fs::read(sidecar_path(&backup, "-shm")).unwrap(),
            b"stale shm"
        );
        // 同一路径建新库可用（残留侧车不会把空库搞坏）
        let store = NoteStore::open(&path).unwrap();
        assert!(store.recovered_from.is_none());
        store.create("fresh").unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn spec_20261107_note__sidecar_move_falls_back_to_copy() {
        // 侧车 rename 失败不等于“侧车搬不走”：rename 要求源和目标同设备、且两个目录都可写
        // （EXDEV / 目录只读都会失败），而“备份到备份名”只需要读源文件、写目标目录。
        // 这里用只读源目录构造确定性的 rename 失败：copy 能成功、remove 会失败。
        // （root 会无视目录权限，CI 与本地都以普通用户运行；只读语义在 Windows 不同，门控掉。）
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        let from = locked.join("note.db-wal");
        std::fs::write(&from, b"stale wal pages").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let to = dir.path().join("note.db.corrupt.1-wal");
        let result = move_sidecar(&from, &to);
        // 先还原权限：TempDir 收尾要写权限
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        // 关键：rename 失败的侧车照样到了备份名，页数据没丢
        assert_eq!(std::fs::read(&to).unwrap(), b"stale wal pages");
        // 原文件删不掉就留在原地，但必须如实上报给调用方 warn，不能静默当作已搬走
        assert!(from.exists());
        let error = result.expect_err("remove_file in a read-only dir must fail");
        assert!(error.contains("rename failed"), "{}", error);
        assert!(error.contains("copy fallback failed"), "{}", error);
    }

    #[test]
    fn spec_20261106_note__corrupt_backups_never_overwrite_previous() {
        // 同一秒内连续两次损坏必须产生两个备份文件：第二次（往往是刚建的空库）
        // 覆盖掉上一份，就等于把真正的旧库扔了。
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.db");
        let mut backups = Vec::new();
        for payload in ["first garbage", "second garbage"] {
            std::fs::write(&path, payload).unwrap();
            let store = NoteStore::open(&path).unwrap();
            backups.push(store.recovered_from.clone().expect("should back up"));
            drop(store);
        }
        assert_ne!(backups[0], backups[1]);
        assert!(backups[0].exists() && backups[1].exists());
        assert_eq!(std::fs::read(&backups[0]).unwrap(), b"first garbage");
        assert_eq!(std::fs::read(&backups[1]).unwrap(), b"second garbage");
    }

    #[test]
    fn spec_20261106_note__reopen_writes_no_schema_change() {
        // 每次打开都 DROP + CREATE trigger 是写事务，多窗口同时启动会互相抢写锁
        // （database is locked）。结构已是最新时打开必须一条 DDL 都不发：
        // schema_version 是 SQLite 的 schema 变更计数，任何 DDL 都会推高它。
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.db");
        fn schema_version(store: &NoteStore) -> i64 {
            store
                .conn
                .lock()
                .query_row("PRAGMA schema_version", [], |r| r.get(0))
                .unwrap()
        }
        let store = NoteStore::open(&path).unwrap();
        store.create("hello world").unwrap();
        let before = schema_version(&store);
        drop(store);

        let reopened = NoteStore::open(&path).unwrap();
        assert!(reopened.recovered_from.is_none());
        assert_eq!(schema_version(&reopened), before, "reopen ran DDL");
        assert_eq!(reopened.list().unwrap().len(), 1);
        assert_eq!(reopened.search("hello").unwrap().len(), 1);
    }

    #[test]
    fn spec_20261106_note__pinned_update_does_not_retokenize_content() {
        // notes_au 带 `OF content`：置顶这类不改正文的 UPDATE 不该重跑分词。
        // 先用定义断言：行为断言失败（FTS5 会报索引不一致）时能给出好读的原因。
        let (store, _dir) = tmp_store();
        let trigger_sql: String = store
            .conn
            .lock()
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='trigger' AND name='notes_au'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            trigger_sql.contains("UPDATE OF content"),
            "notes_au must fire on content only: {}",
            trigger_sql
        );
        // 再验行为：用一个故意打乱的索引来观测，置顶后索引仍是乱的 = trigger 没被触发。
        let note = store.create("hello world").unwrap();
        store
            .conn
            .lock()
            .execute(
                "INSERT INTO notes_fts(notes_fts, rowid, content) VALUES ('delete', ?1, ?2)",
                params![note.id, note.content.as_str()],
            )
            .unwrap();
        assert!(store.search_fts(&search_terms("hello")).unwrap().is_empty());
        store.set_pinned(note.id, true).unwrap();
        // 不带 `OF content` 时，置顶会让 trigger 把正文重新写回索引
        assert!(store.search_fts(&search_terms("hello")).unwrap().is_empty());
        // 改正文照常同步索引（换一条没被手动去同步的行，否则 FTS5 会报 image is malformed）
        let other = store.create("second note").unwrap();
        store.update(other.id, "second shifted").unwrap();
        assert_eq!(store.search_fts(&search_terms("shifted")).unwrap().len(), 1);
    }

    #[test]
    fn spec_20261106_note__write_paths_ignore_soft_deleted_rows() {
        // 已软删的行是“用户看不见的”，写路径也不许改它：否则看不见的文本被改写，
        // 之后 restore 出来的是被偷偷改过的内容。
        let (store, _dir) = tmp_store();
        let note = store.create("original").unwrap();
        store.delete(note.id).unwrap();
        assert_eq!(store.update(note.id, "hijacked").unwrap_err(), "not_found");
        assert_eq!(store.set_pinned(note.id, true).unwrap_err(), "not_found");
        assert_eq!(store.get(note.id).unwrap().unwrap().content, "original");
        store.restore(note.id).unwrap();
        assert!(!store.list().unwrap()[0].pinned);
        // 未删除的行照常可写
        assert_eq!(store.update(note.id, "edited").unwrap().content, "edited");
        store.set_pinned(note.id, true).unwrap();
        assert!(store.list().unwrap()[0].pinned);
    }

    #[test]
    fn spec_20261106_note__purge_deleted_zero_clears_immediately() {
        // cutoff 与 deleted_at 落在同一秒时也必须清掉：`<` 会留一行在库里，
        // 而 `purge_deleted(0)` 的语义就是“现在就清干净”。
        let (store, _dir) = tmp_store();
        let note = store.create("bye").unwrap();
        store.delete(note.id).unwrap();
        assert_eq!(store.purge_deleted(0).unwrap(), 1);
        assert!(store.get(note.id).unwrap().is_none());
        assert!(store.search_like(&search_terms("bye")).unwrap().is_empty());
    }

    #[test]
    fn spec_20260827_note__wal_and_user_version() {
        let (store, _dir) = tmp_store();
        let conn = store.conn.lock();
        let wal: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(wal.to_lowercase(), "wal");
        let ver: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ver, SCHEMA_VERSION);
    }

    #[test]
    fn spec_20261107_note__open_sets_busy_timeout() {
        // 两窗口同时首启时 `ensure_schema` 里的 `PRAGMA journal_mode=WAL` 要拿排他锁，
        // busy_timeout 为 0 会立刻 SQLITE_BUSY，被归为 NotDamaged，用户看到“笔记库打开失败”。
        // `PRAGMA busy_timeout` 回毫秒，未设置时为 0——它才是“真设过”的证据。
        let (store, _dir) = tmp_store();
        let timeout: i64 = store
            .conn
            .lock()
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(timeout, 5_000);
    }

    #[test]
    fn spec_20261107_note__only_notadb_and_corrupt_count_as_damaged() {
        // 判定表：只有“文件内容不是数据库”才允许洗库。
        // busy / cantopen / ioerr / readonly / nomem / diskfull 都能穿过同一条探测路径，
        // 把它们也当损坏就是替用户改名搬走库、再换成空库。
        // 这里断言的是 `classify_open_error` 的分类契约（判定表），不是生产路径：
        // 真实错误码由 SQLite 在锁竞争、磁盘满等情形下给出，无法在单测里逐条稳定构造。
        use rusqlite::ffi::{Error as FfiError, ErrorCode};
        let failure = |code: ErrorCode| {
            classify_open_error(rusqlite::Error::SqliteFailure(
                FfiError {
                    code,
                    extended_code: 0,
                },
                None,
            ))
        };
        for code in [ErrorCode::NotADatabase, ErrorCode::DatabaseCorrupt] {
            assert!(
                matches!(failure(code), OpenFailure::Corrupt(_)),
                "{:?} must count as damaged",
                code
            );
        }
        for code in [
            ErrorCode::DatabaseBusy,
            ErrorCode::DatabaseLocked,
            ErrorCode::CannotOpen,
            ErrorCode::SystemIoFailure,
            ErrorCode::ReadOnly,
            ErrorCode::OutOfMemory,
            ErrorCode::DiskFull,
            // 只读库上 `PRAGMA journal_mode=WAL` 报的就是它：上抛，不搬库
            ErrorCode::PermissionDenied,
        ] {
            assert!(
                matches!(failure(code), OpenFailure::NotDamaged(_)),
                "{:?} must not wipe the library",
                code
            );
        }
        // 非 SqliteFailure（拿不到错误码）一律不算损坏：宁可打不开，也不许洗库
        assert!(matches!(
            classify_open_error(rusqlite::Error::QueryReturnedNoRows),
            OpenFailure::NotDamaged(_)
        ));
    }

    #[test]
    fn spec_20261107_note__unopenable_path_is_not_wiped() {
        // `note.db` 是目录时 `Connection::open` 返回 SQLITE_CANTOPEN——文件不是“打不开”，
        // 而是“根本不是一个库”。旧实现把它当损坏：目录被 rename 走、旁边建一个新的空库，
        // 用户既看不到旧库也不知道发生了什么。
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.db");
        std::fs::create_dir(&path).unwrap();
        let err = match NoteStore::open(&path) {
            Ok(_) => panic!("a directory must not open as a note database"),
            Err(e) => e,
        };
        assert!(!err.is_empty());
        assert!(path.is_dir(), "cantopen moved the path away: {}", err);
        let backups = corrupt_backups(dir.path());
        assert!(
            backups.is_empty(),
            "cantopen wiped the library: {:?}",
            backups
        );
    }

    #[cfg(unix)]
    #[test]
    fn spec_20261107_note__readonly_db_is_not_treated_as_damaged() {
        // 只读文件不是“文件内容坏了”：rename 只要求目录可写、不要求文件本身可写，
        // 所以把 readonly 判成损坏的实现能把一个完好的库搬走再换成空库。
        // 这条用例守三件事：
        //   1. open 成功：SQLite 在 READWRITE 打开失败时会回退到只读连接，所以“只读库只在
        //      写时才失败”是预期行为而非缺陷（此处 schema 已是当前版本，一条 DDL 都不发）；
        //   2. `path` 仍在原地：一个字节都没被搬走；
        //   3. 目录里零个 `.corrupt.*`、`user_version` 没变：没洗库。
        // 只读语义在 Windows 不同（文件属性 vs 权限位），这里门控掉。
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.db");
        {
            let store = NoteStore::open(&path).unwrap();
            store.create("keep me").unwrap();
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let opened = NoteStore::open(&path);
        // 先还原权限：后续断言（`user_version_of` 会开新连接）与 TempDir 收尾都要写权限
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let store = match opened {
            Ok(store) => store,
            Err(e) => panic!("a read-only db must still open, got error: {}", e),
        };
        assert!(
            store.recovered_from.is_none(),
            "read-only db was backed up away"
        );
        assert!(path.exists());
        assert_eq!(user_version_of(&path), SCHEMA_VERSION);
        assert!(
            corrupt_backups(dir.path()).is_empty(),
            "readonly error wiped the db: {:?}",
            corrupt_backups(dir.path())
        );
        // 旧数据读得到；写则明确报 readonly，而不是静默失败或重建
        assert_eq!(store.list().unwrap().len(), 1);
        let write_error = store
            .create("nope")
            .expect_err("a read-only db must reject writes");
        assert!(
            write_error.to_lowercase().contains("readonly"),
            "unexpected write error: {}",
            write_error
        );
        // 写失败后库还是那份
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn spec_20261107_note__newer_schema_version_is_refused_not_downgraded() {
        // 重建条件用 `version < SCHEMA_VERSION`，末尾却无条件写版本号。
        // 遇到更高版本的库不挡住，就会把它静默降级标成旧版（元数据改坏、结构却没降）。
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.db");
        let newer = SCHEMA_VERSION + 1;
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&format!("PRAGMA user_version={};", newer))
                .unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE notes (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    content TEXT NOT NULL,
                    pinned INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    deleted_at INTEGER
                );
                INSERT INTO notes (content, created_at, updated_at) VALUES ('future', 1, 1);
                "#,
            )
            .unwrap();
        }
        let err = match NoteStore::open(&path) {
            Ok(_) => panic!("a newer schema must not be migrated"),
            Err(e) => e,
        };
        assert!(err.contains("newer"), "unexpected error: {}", err);
        // 版本号没被降级写回，迁移也没开始（notes_fts / trigger 一个都没建）
        assert_eq!(user_version_of(&path), newer);
        let conn = Connection::open(&path).unwrap();
        let objects: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name IN ('notes_fts','notes_ai','notes_au')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(objects, 0, "newer db was migrated");
        assert!(corrupt_backups(dir.path()).is_empty());
    }

    #[test]
    fn spec_20261107_note__older_version_rebuilds_trigger_definitions() {
        // `schema_is_current` 只看 trigger 名字在不在，`notes_au` 定义改了它不会变 false。
        // 版本不 +1，已 user_version=2 的库就会永远留着旧的 `AFTER UPDATE ON notes`。
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE notes (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    content TEXT NOT NULL,
                    pinned INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    deleted_at INTEGER
                );
                CREATE VIRTUAL TABLE notes_fts USING fts5(content, content='notes', content_rowid='id', tokenize='unicode61');
                CREATE TRIGGER notes_ai AFTER INSERT ON notes BEGIN
                    INSERT INTO notes_fts(rowid, content) VALUES (new.id, new.content);
                END;
                CREATE TRIGGER notes_ad AFTER DELETE ON notes BEGIN
                    INSERT INTO notes_fts(notes_fts, rowid, content) VALUES ('delete', old.id, old.content);
                END;
                CREATE TRIGGER notes_au AFTER UPDATE ON notes BEGIN
                    INSERT INTO notes_fts(notes_fts, rowid, content) VALUES ('delete', old.id, old.content);
                    INSERT INTO notes_fts(rowid, content) VALUES (new.id, new.content);
                END;
                INSERT INTO notes (content, created_at, updated_at) VALUES ('legacy two', 1, 1);
                PRAGMA user_version=2;
                "#,
            )
            .unwrap();
        }
        let store = NoteStore::open(&path).unwrap();
        assert!(store.recovered_from.is_none());
        let conn = store.conn.lock();
        let ver: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ver, SCHEMA_VERSION);
        let trigger_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='trigger' AND name='notes_au'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            trigger_sql.contains("UPDATE OF content"),
            "stale trigger definition kept: {}",
            trigger_sql
        );
        drop(conn);
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn spec_20261107_note__residual_tags_column_does_not_rerun_ddl() {
        // `DROP COLUMN tags` 失败只 warn、版本号照写。若 `schema_is_current` 仍把残留
        // tags 判为 not current，这类库每次启动都要 DROP/CREATE trigger + 写版本号，
        // 又回到多窗口抢写锁。这里模拟一个 drop 失败的库：版本已是当前，tags 还在。
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.db");
        {
            let store = NoteStore::open(&path).unwrap();
            store.create("hello").unwrap();
            store
                .conn
                .lock()
                .execute("ALTER TABLE notes ADD COLUMN tags TEXT", [])
                .unwrap();
        }
        let before = schema_version_of(&path);
        let reopened = NoteStore::open(&path).unwrap();
        assert!(reopened.recovered_from.is_none());
        assert_eq!(
            schema_version_of(&path),
            before,
            "residual tags column reran DDL"
        );
        assert_eq!(reopened.list().unwrap().len(), 1);
        assert_eq!(user_version_of(&path), SCHEMA_VERSION);
    }

    #[test]
    fn spec_20261107_note__column_check_is_case_insensitive() {
        // SQLite 列名不区分大小写。库里叫 `Pinned` 时用 `==` 会判成缺列，
        // 接着 `ADD COLUMN pinned` 撞 `duplicate column name`（补列是硬错），整库打不开。
        // 仅防御性：本程序所有 DDL 都用小写列名，生产不可达——但脏数据 / 外部工具改过
        // 表结构的库会命中，且代价是“整库打不开”，所以保留这个覆盖。
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&format!(
                r#"
                PRAGMA user_version={};
                CREATE TABLE notes (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    content TEXT NOT NULL,
                    Pinned INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    Deleted_At INTEGER
                );
                INSERT INTO notes (content, Pinned, created_at, updated_at) VALUES ('mixed case', 1, 1, 1);
                "#,
                SCHEMA_VERSION
            ))
            .unwrap();
        }
        let store = NoteStore::open(&path).unwrap();
        assert!(store.recovered_from.is_none());
        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].pinned);
        store.create("new note").unwrap();
        assert_eq!(store.list().unwrap().len(), 2);
        assert!(corrupt_backups(dir.path()).is_empty());
    }
}
