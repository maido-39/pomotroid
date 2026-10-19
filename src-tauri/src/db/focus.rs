//! Tasks, round notes and the daily goal (MIGRATION_17).
//!
//! A task is a name a focus round is recorded against — what other apps call
//! a project or a tag. Tasks are archived rather than deleted when they still
//! have rounds, so last month's statistics keep their names; one with no
//! rounds can be removed outright.
use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::{Deserialize, Serialize};

use super::queries::{compute_streak, StreakInfo};

/// Longest task name, in characters.
pub const MAX_TASK_NAME: usize = 60;
/// Longest note, in characters. A note is a line, not a journal entry.
pub const MAX_NOTE: usize = 280;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Task {
    pub id: i64,
    pub name: String,
    /// `#rrggbb`, or empty for the theme's accent.
    pub color: String,
    pub archived: bool,
}

fn clean_name(name: &str) -> Result<String, String> {
    let name: String = name.trim().chars().take(MAX_TASK_NAME).collect();
    if name.is_empty() {
        return Err("a task needs a name".into());
    }
    Ok(name)
}

fn clean_color(color: &str) -> String {
    let c = color.trim();
    let hex = c.strip_prefix('#').unwrap_or("");
    if hex.len() == 6 && hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        c.to_ascii_lowercase()
    } else {
        String::new()
    }
}

fn row_to_task(r: &rusqlite::Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: r.get(0)?,
        name: r.get(1)?,
        color: r.get(2)?,
        archived: r.get::<_, i64>(3)? != 0,
    })
}

/// Tasks in the order they were created; archived ones only when asked for.
pub fn list_tasks(conn: &Connection, include_archived: bool) -> Result<Vec<Task>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, color, archived FROM tasks
         WHERE archived = 0 OR ?1
         ORDER BY id",
    )?;
    let rows = stmt.query_map([include_archived], row_to_task)?.collect();
    rows
}

pub fn get_task(conn: &Connection, id: i64) -> Result<Option<Task>> {
    conn.query_row(
        "SELECT id, name, color, archived FROM tasks WHERE id = ?1",
        [id],
        row_to_task,
    )
    .optional()
}

/// Create a task. A name that already exists (in any case) brings that task
/// back from the archive instead of failing, which is what someone typing it
/// again means.
pub fn create_task(conn: &Connection, name: &str, color: &str) -> std::result::Result<Task, String> {
    let name = clean_name(name)?;
    let color = clean_color(color);
    let existing: Option<i64> = conn
        .query_row("SELECT id FROM tasks WHERE name = ?1", [&name], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(id) = existing {
        conn.execute("UPDATE tasks SET archived = 0 WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
        return get_task(conn, id).map_err(|e| e.to_string())?.ok_or_else(|| "task vanished".into());
    }
    conn.execute(
        "INSERT INTO tasks (name, color, archived, created_at) VALUES (?1, ?2, 0, strftime('%s','now'))",
        params![name, color],
    )
    .map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    Ok(Task { id, name, color, archived: false })
}

pub fn update_task(conn: &Connection, id: i64, name: &str, color: &str) -> std::result::Result<Task, String> {
    let name = clean_name(name)?;
    let color = clean_color(color);
    let clash: Option<i64> = conn
        .query_row("SELECT id FROM tasks WHERE name = ?1 AND id != ?2", params![name, id], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    if clash.is_some() {
        return Err(format!("a task called \"{name}\" already exists"));
    }
    let n = conn
        .execute("UPDATE tasks SET name = ?1, color = ?2 WHERE id = ?3", params![name, color, id])
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("no such task".into());
    }
    get_task(conn, id).map_err(|e| e.to_string())?.ok_or_else(|| "no such task".into())
}

/// Remove a task from the switcher. A task that has rounds is archived so the
/// statistics keep its name; one without any is deleted. Returns true when
/// the row was deleted.
pub fn remove_task(conn: &Connection, id: i64) -> Result<bool> {
    let used: i64 = conn.query_row("SELECT COUNT(*) FROM sessions WHERE task_id = ?1", [id], |r| r.get(0))?;
    if used > 0 {
        conn.execute("UPDATE tasks SET archived = 1 WHERE id = ?1", [id])?;
        Ok(false)
    } else {
        conn.execute("DELETE FROM tasks WHERE id = ?1", [id])?;
        Ok(true)
    }
}

/// Attach `note` to the focus round that ended most recently. Empty clears
/// it. Returns false when there is no finished focus round to attach it to.
pub fn set_last_note(conn: &Connection, note: &str) -> Result<bool> {
    let note: String = note.trim().chars().take(MAX_NOTE).collect();
    let value: Option<&str> = if note.is_empty() { None } else { Some(&note) };
    let n = conn.execute(
        "UPDATE sessions SET note = ?1
         WHERE id = (SELECT id FROM sessions
                     WHERE round_type = 'work' AND ended_at IS NOT NULL
                     ORDER BY ended_at DESC, id DESC LIMIT 1)",
        params![value],
    )?;
    Ok(n > 0)
}

/// One recorded round, as the day view lists it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub round_type: String,
    pub duration_secs: u32,
    pub completed: bool,
    pub task: Option<String>,
    pub task_color: String,
    pub note: Option<String>,
}

/// Focus rounds that ended on a local calendar day ("YYYY-MM-DD"), newest
/// first. Rounds abandoned by a reset (no end) are left out.
pub fn sessions_on(conn: &Connection, day: &str) -> Result<Vec<SessionEntry>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.started_at, s.ended_at, s.round_type, s.duration_secs, s.completed,
                t.name, COALESCE(t.color, ''), s.note
         FROM sessions s LEFT JOIN tasks t ON t.id = s.task_id
         WHERE s.round_type = 'work' AND s.ended_at IS NOT NULL
           AND date(s.started_at, 'unixepoch', 'localtime') = ?1
         ORDER BY s.started_at DESC, s.id DESC",
    )?;
    let rows = stmt
        .query_map([day], |r| {
            Ok(SessionEntry {
                id: r.get(0)?,
                started_at: r.get(1)?,
                ended_at: r.get(2)?,
                round_type: r.get(3)?,
                duration_secs: r.get(4)?,
                completed: r.get::<_, i64>(5)? != 0,
                task: r.get(6)?,
                task_color: r.get(7)?,
                note: r.get(8)?,
            })
        })?
        .collect();
    rows
}

/// Focus time per task over a span ending today.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskTotal {
    /// None for rounds recorded without a task.
    pub task_id: Option<i64>,
    pub name: Option<String>,
    pub color: String,
    pub rounds: u32,
    pub focus_secs: u32,
}

/// `days` = 1 for today, 7 for the last week, 0 for all time.
pub fn task_totals(conn: &Connection, days: u32) -> Result<Vec<TaskTotal>> {
    let mut stmt = conn.prepare(
        "SELECT s.task_id, t.name, COALESCE(t.color, ''), COUNT(*), COALESCE(SUM(s.duration_secs), 0)
         FROM sessions s LEFT JOIN tasks t ON t.id = s.task_id
         WHERE s.round_type = 'work' AND s.completed = 1
           AND (?1 = 0 OR date(s.started_at, 'unixepoch', 'localtime')
                         >= date('now', 'localtime', printf('-%d days', ?1 - 1)))
         GROUP BY s.task_id
         ORDER BY 5 DESC",
    )?;
    let rows = stmt
        .query_map([days], |r| {
            Ok(TaskTotal {
                task_id: r.get(0)?,
                name: r.get(1)?,
                color: r.get(2)?,
                rounds: r.get(3)?,
                focus_secs: r.get(4)?,
            })
        })?
        .collect();
    rows
}

/// Today's progress against the daily goal, and the goal streak.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalProgress {
    /// The goal it was measured against; 0 means no goal is set.
    pub goal: u32,
    /// Completed focus rounds today.
    pub today: u32,
    /// Consecutive days that met the goal. With no goal, consecutive days with
    /// any completed focus round — the streak the statistics already showed.
    pub streak: StreakInfo,
}

pub fn goal_progress(conn: &Connection, goal: u32) -> Result<GoalProgress> {
    let today_date: String = conn.query_row("SELECT date('now', 'localtime')", [], |r| r.get(0))?;
    let today: u32 = conn.query_row(
        "SELECT COUNT(*) FROM sessions
         WHERE round_type = 'work' AND completed = 1
           AND date(started_at, 'unixepoch', 'localtime') = ?1",
        [&today_date],
        |r| r.get(0),
    )?;
    let mut stmt = conn.prepare(
        "SELECT date(started_at, 'unixepoch', 'localtime') AS day
         FROM sessions
         WHERE round_type = 'work' AND completed = 1
         GROUP BY day
         HAVING COUNT(*) >= ?1
         ORDER BY day",
    )?;
    let days: Vec<String> = stmt.query_map([goal.max(1)], |r| r.get(0))?.flatten().collect();
    Ok(GoalProgress { goal, today, streak: compute_streak(&days, &today_date) })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{migrations, queries};

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        migrations::run(&conn).unwrap();
        conn
    }

    fn finished(conn: &Connection, task: Option<i64>, secs: u32, completed: bool) -> i64 {
        let id = queries::insert_session_for(conn, "work", secs, task).unwrap();
        queries::finish_session(conn, id, completed, None).unwrap();
        id
    }

    #[test]
    fn creating_a_name_twice_revives_the_task() {
        let conn = setup();
        let a = create_task(&conn, "  Thesis  ", "#ABCDEF").unwrap();
        assert_eq!(a.name, "Thesis");
        assert_eq!(a.color, "#abcdef");
        finished(&conn, Some(a.id), 1500, true);
        assert!(!remove_task(&conn, a.id).unwrap(), "a used task is archived");
        assert!(list_tasks(&conn, false).unwrap().is_empty());
        let again = create_task(&conn, "thesis", "").unwrap();
        assert_eq!(again.id, a.id);
        assert!(!again.archived);
    }

    #[test]
    fn an_unused_task_is_deleted_and_bad_input_refused() {
        let conn = setup();
        let t = create_task(&conn, "Reading", "red").unwrap();
        assert_eq!(t.color, "", "only #rrggbb colours are kept");
        assert!(remove_task(&conn, t.id).unwrap());
        assert!(get_task(&conn, t.id).unwrap().is_none());
        assert!(create_task(&conn, "   ", "").is_err());
        let a = create_task(&conn, "A", "").unwrap();
        create_task(&conn, "B", "").unwrap();
        assert!(update_task(&conn, a.id, "b", "").is_err(), "names stay unique in any case");
    }

    #[test]
    fn deleting_a_task_unlinks_its_rounds() {
        let conn = setup();
        let t = create_task(&conn, "Temp", "").unwrap();
        // Rounds are attached afterwards, as if the task had been chosen
        // after it was removed from another device.
        let id = finished(&conn, None, 1500, true);
        conn.execute("UPDATE sessions SET task_id = ?1 WHERE id = ?2", [t.id, id]).unwrap();
        conn.execute("DELETE FROM tasks WHERE id = ?1", [t.id]).unwrap();
        let task: Option<i64> = conn.query_row("SELECT task_id FROM sessions WHERE id = ?1", [id], |r| r.get(0)).unwrap();
        assert_eq!(task, None);
    }

    #[test]
    fn notes_attach_to_the_latest_finished_focus_round() {
        let conn = setup();
        assert!(!set_last_note(&conn, "nothing yet").unwrap());
        let first = finished(&conn, None, 1500, true);
        conn.execute("UPDATE sessions SET ended_at = ended_at - 100 WHERE id = ?1", [first]).unwrap();
        let second = finished(&conn, None, 1500, false);
        // A break in progress must not take the note.
        queries::insert_session(&conn, "short-break", 300).unwrap();
        assert!(set_last_note(&conn, &"x".repeat(400)).unwrap());
        let note: Option<String> = conn.query_row("SELECT note FROM sessions WHERE id = ?1", [second], |r| r.get(0)).unwrap();
        assert_eq!(note.unwrap().chars().count(), MAX_NOTE);
        assert!(set_last_note(&conn, "  ").unwrap());
        let cleared: Option<String> = conn.query_row("SELECT note FROM sessions WHERE id = ?1", [second], |r| r.get(0)).unwrap();
        assert_eq!(cleared, None);
    }

    #[test]
    fn totals_group_by_task_and_count_completed_rounds_only() {
        let conn = setup();
        let t = create_task(&conn, "Code", "#112233").unwrap();
        finished(&conn, Some(t.id), 1500, true);
        finished(&conn, Some(t.id), 1500, true);
        finished(&conn, Some(t.id), 1500, false);
        finished(&conn, None, 600, true);
        let totals = task_totals(&conn, 1).unwrap();
        assert_eq!(totals.len(), 2);
        assert_eq!(totals[0].name.as_deref(), Some("Code"));
        assert_eq!((totals[0].rounds, totals[0].focus_secs), (2, 3000));
        assert_eq!((totals[1].task_id, totals[1].rounds), (None, 1));
        let day: String = conn.query_row("SELECT date('now','localtime')", [], |r| r.get(0)).unwrap();
        let listed = sessions_on(&conn, &day).unwrap();
        assert_eq!(listed.len(), 4, "skipped rounds are listed too, marked incomplete");
        assert_eq!(listed.iter().filter(|s| !s.completed).count(), 1);
    }

    #[test]
    fn goal_progress_counts_today_and_goal_days() {
        let conn = setup();
        for _ in 0..3 {
            finished(&conn, None, 1500, true);
        }
        // Yesterday: two rounds.
        for _ in 0..2 {
            let id = finished(&conn, None, 1500, true);
            conn.execute("UPDATE sessions SET started_at = started_at - 86400 WHERE id = ?1", [id]).unwrap();
        }
        let g = goal_progress(&conn, 3).unwrap();
        assert_eq!((g.goal, g.today), (3, 3));
        assert_eq!(g.streak.current, 1, "yesterday missed the goal of three");
        let g2 = goal_progress(&conn, 2).unwrap();
        assert_eq!(g2.streak.current, 2);
        let none = goal_progress(&conn, 0).unwrap();
        assert_eq!(none.streak.current, 2, "no goal means any focus round counts");
    }

    #[test]
    fn flow_rounds_record_the_time_actually_spent() {
        let conn = setup();
        let id = queries::insert_session_for(&conn, "work", 1500, None).unwrap();
        queries::finish_session(&conn, id, true, Some(1980)).unwrap();
        let secs: u32 = conn.query_row("SELECT duration_secs FROM sessions WHERE id = ?1", [id], |r| r.get(0)).unwrap();
        assert_eq!(secs, 1980);
    }
}
