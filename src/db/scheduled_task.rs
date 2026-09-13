use super::{Database, join_error, open_connection, sql_error};
use crate::{
    AppError,
    scheduled_task::{HttpConfig, Run, Task, TaskRequest},
};
use chrono::Utc;
use rusqlite::{OptionalExtension, params};

fn invalid(message: impl Into<String>) -> AppError {
    AppError::InvalidConfig {
        message: message.into(),
    }
}
impl Database {
    async fn scheduled_db<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut rusqlite::Connection) -> Result<T, AppError> + Send + 'static,
    ) -> Result<T, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || f(&mut open_connection(&path)?))
            .await
            .map_err(join_error)?
    }
    pub async fn scheduled_http_config(&self, id: i64) -> Result<HttpConfig, AppError> {
        self.scheduled_db(move |conn| {
            get_task(conn, id)?
                .http
                .ok_or_else(|| invalid("请求配置不存在"))
        })
        .await
    }
    pub async fn init_scheduled_tasks(&self) -> Result<(), AppError> {
        self.scheduled_db(|conn| {
            conn.execute_batch("CREATE TABLE IF NOT EXISTS scheduled_tasks (
                id INTEGER PRIMARY KEY, config TEXT NOT NULL, http TEXT NOT NULL,
                enabled INTEGER NOT NULL, next_run_at TEXT, running INTEGER NOT NULL DEFAULT 0);
                CREATE TABLE IF NOT EXISTS scheduled_task_runs (
                id INTEGER PRIMARY KEY, task_id INTEGER NOT NULL REFERENCES scheduled_tasks(id) ON DELETE CASCADE,
                record TEXT NOT NULL);
                CREATE INDEX IF NOT EXISTS scheduled_runs_task ON scheduled_task_runs(task_id, id);").map_err(sql_error)?;
            Ok(())
        }).await
    }
    pub async fn recover_scheduled_tasks(&self) -> Result<(), AppError> {
        self.scheduled_db(|conn| {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let records = {
                let mut statement = tx
                    .prepare("SELECT id,record FROM scheduled_task_runs")
                    .map_err(sql_error)?;
                statement
                    .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                    .map_err(sql_error)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(sql_error)?
            };
            for (id, json) in records {
                let mut run: Run =
                    serde_json::from_str(&json).map_err(|_| invalid("执行记录损坏"))?;
                if run.status == "running" {
                    run.status = "interrupted".into();
                    run.finished_at = Some(Utc::now().to_rfc3339());
                    run.message = "服务重启，执行结果未知；不会自动重试".into();
                    tx.execute(
                        "UPDATE scheduled_task_runs SET record=? WHERE id=?",
                        params![serde_json::to_string(&run).unwrap(), id],
                    )
                    .map_err(sql_error)?;
                }
            }
            tx.execute("UPDATE scheduled_tasks SET running=0", [])
                .map_err(sql_error)?;
            tx.commit().map_err(sql_error)
        })
        .await
    }
    pub async fn list_scheduled_tasks(&self) -> Result<Vec<Task>, AppError> {
        self.scheduled_db(|conn| {
            let mut stmt = conn.prepare("SELECT id, config, http, enabled, next_run_at, running FROM scheduled_tasks ORDER BY id DESC").map_err(sql_error)?;
            let mut tasks = stmt.query_map([], read_task).map_err(sql_error)?.collect::<Result<Vec<_>, _>>().map_err(sql_error)?;
            for task in &mut tasks { task.last_run = latest_run(conn, task.id)?; }
            Ok(tasks)
        }).await
    }
    pub async fn save_scheduled_task(
        &self,
        id: Option<i64>,
        body: TaskRequest,
    ) -> Result<i64, AppError> {
        self.scheduled_db(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(sql_error)?;
            let old = if let Some(id) = id { Some(get_task(&tx, id)?) } else { None };
            if old.as_ref().is_some_and(|t| t.running) { return Err(invalid("任务执行中，请等待完成后修改")); }
            let http = body.http.or_else(|| old.as_ref().and_then(|t| t.http.clone())).ok_or_else(|| invalid("请配置 HTTP 请求"))?;
            http.validate().map_err(invalid)?;
            let next = body.timing.next(Utc::now()).map_err(invalid)?;
            if body.name.trim().is_empty() || body.name.chars().count() > 100 { return Err(invalid("任务名称应为 1–100 个字符")); }
            if body.task_type != "http" { return Err(invalid("暂不支持此任务类型")); }
            let config = Task { id: id.unwrap_or(0), name: body.name.trim().into(), task_type: body.task_type, enabled: body.enabled, timing: body.timing,
                request_summary: http.summary(), next_run_at: None, running: false, last_run: None, http: None };
            let config_json = serde_json::to_string(&config).unwrap();
            let http_json = serde_json::to_string(&http).unwrap();
            let next_at = body.enabled.then(|| next.to_rfc3339());
            let id = if let Some(id) = id {
                tx.execute("UPDATE scheduled_tasks SET config=?, http=?, enabled=?, next_run_at=? WHERE id=?", params![config_json, http_json, body.enabled, next_at, id]).map_err(sql_error)?; id
            } else {
                tx.execute("INSERT INTO scheduled_tasks(config,http,enabled,next_run_at) VALUES(?,?,?,?)", params![config_json,http_json,body.enabled,next_at]).map_err(sql_error)?; tx.last_insert_rowid()
            };
            tx.commit().map_err(sql_error)?; Ok(id)
        }).await
    }
    pub async fn set_scheduled_enabled(&self, id: i64, enabled: bool) -> Result<(), AppError> {
        self.scheduled_db(move |conn| {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let task = get_task(&tx, id)?;
            let next = if enabled {
                Some(task.timing.next(Utc::now()).map_err(invalid)?.to_rfc3339())
            } else {
                None
            };
            tx.execute(
                "UPDATE scheduled_tasks SET enabled=?,next_run_at=? WHERE id=?",
                params![enabled, next, id],
            )
            .map_err(sql_error)?;
            tx.commit().map_err(sql_error)
        })
        .await
    }
    pub async fn delete_scheduled_task(&self, id: i64) -> Result<(), AppError> {
        self.scheduled_db(move |conn| {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let task = get_task(&tx, id)?;
            if task.running {
                return Err(invalid("任务执行中，请等待完成后删除"));
            }
            tx.execute("DELETE FROM scheduled_task_runs WHERE task_id=?", [id])
                .map_err(sql_error)?;
            tx.execute("DELETE FROM scheduled_tasks WHERE id=?", [id])
                .map_err(sql_error)?;
            tx.commit().map_err(sql_error)
        })
        .await
    }
    pub async fn claim_scheduled_task(
        &self,
        id: i64,
        manual: bool,
    ) -> Result<(Task, Run), AppError> {
        self.scheduled_db(move |conn| {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let task = get_task(&tx, id)?;
            let now = Utc::now();
            if task.running {
                return Err(invalid("任务正在执行，请等待完成"));
            }
            if !manual
                && (!task.enabled
                    || task
                        .next_run_at
                        .as_deref()
                        .is_none_or(|v| v > now.to_rfc3339().as_str()))
            {
                return Err(invalid("任务未到执行时间"));
            }
            // Automatic claims advance from now, skipping missed ticks. Manual runs preserve the schedule.
            let next = if manual {
                task.next_run_at.clone()
            } else {
                task.timing.next(now).ok().map(|v| v.to_rfc3339())
            };
            tx.execute(
                "UPDATE scheduled_tasks SET running=1,next_run_at=? WHERE id=?",
                params![next, id],
            )
            .map_err(sql_error)?;
            let mut run = Run {
                id: 0,
                task_id: id,
                trigger: if manual { "manual" } else { "automatic" }.into(),
                started_at: now.to_rfc3339(),
                finished_at: None,
                status: "running".into(),
                status_code: None,
                duration_ms: None,
                message: "正在发送请求".into(),
            };
            tx.execute(
                "INSERT INTO scheduled_task_runs(task_id,record) VALUES(?,?)",
                params![id, serde_json::to_string(&run).unwrap()],
            )
            .map_err(sql_error)?;
            run.id = tx.last_insert_rowid();
            tx.execute(
                "UPDATE scheduled_task_runs SET record=? WHERE id=?",
                params![serde_json::to_string(&run).unwrap(), run.id],
            )
            .map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;
            Ok((task, run))
        })
        .await
    }
    pub async fn finish_scheduled_run(
        &self,
        run_id: i64,
        task_id: i64,
        status: String,
        code: Option<u16>,
        duration_ms: u64,
        message: String,
    ) -> Result<(), AppError> {
        self.scheduled_db(move |conn| {
            let tx = conn.transaction().map_err(sql_error)?;
            let json: String = tx.query_row("SELECT record FROM scheduled_task_runs WHERE id=?", [run_id], |r| r.get(0)).map_err(sql_error)?;
            let mut run: Run = serde_json::from_str(&json).map_err(|_| invalid("执行记录损坏"))?;
            run.finished_at = Some(Utc::now().to_rfc3339()); run.status = status; run.status_code = code; run.duration_ms = Some(duration_ms); run.message = message;
            tx.execute("UPDATE scheduled_task_runs SET record=? WHERE id=?", params![serde_json::to_string(&run).unwrap(), run_id]).map_err(sql_error)?;
            tx.execute("UPDATE scheduled_tasks SET running=0 WHERE id=?", [task_id]).map_err(sql_error)?;
            tx.execute("DELETE FROM scheduled_task_runs WHERE task_id=? AND id NOT IN (SELECT id FROM scheduled_task_runs WHERE task_id=? ORDER BY id DESC LIMIT 100)", params![task_id,task_id]).map_err(sql_error)?;
            tx.commit().map_err(sql_error)
        }).await
    }
    pub async fn scheduled_runs(&self, id: i64) -> Result<Vec<Run>, AppError> {
        self.scheduled_db(move |conn| {
            get_task(conn,id)?;
            let mut stmt = conn.prepare("SELECT record FROM scheduled_task_runs WHERE task_id=? ORDER BY id DESC LIMIT 100").map_err(sql_error)?;
            let json = stmt.query_map([id], |r| r.get::<_,String>(0)).map_err(sql_error)?.collect::<Result<Vec<_>,_>>().map_err(sql_error)?;
            json.into_iter().map(|j| serde_json::from_str(&j).map_err(|_| invalid("执行记录损坏"))).collect()
        }).await
    }
}
fn read_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let decode = |index: usize| -> rusqlite::Result<String> { row.get(index) };
    let mut task: Task = serde_json::from_str(&decode(1)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let http: HttpConfig = serde_json::from_str(&decode(2)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
    })?;
    task.id = row.get(0)?;
    task.enabled = row.get(3)?;
    task.next_run_at = row.get(4)?;
    task.running = row.get(5)?;
    task.http = Some(http);
    Ok(task)
}
fn get_task(conn: &rusqlite::Connection, id: i64) -> Result<Task, AppError> {
    conn.query_row(
        "SELECT id,config,http,enabled,next_run_at,running FROM scheduled_tasks WHERE id=?",
        [id],
        read_task,
    )
    .optional()
    .map_err(sql_error)?
    .ok_or_else(|| invalid("定时任务不存在"))
}
fn latest_run(conn: &rusqlite::Connection, id: i64) -> Result<Option<Run>, AppError> {
    let json: Option<String> = conn
        .query_row(
            "SELECT record FROM scheduled_task_runs WHERE task_id=? ORDER BY id DESC LIMIT 1",
            [id],
            |r| r.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    json.map(|j| serde_json::from_str(&j).map_err(|_| invalid("执行记录损坏")))
        .transpose()
}
