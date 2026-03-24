# Phase 7: 可観測性 + 診断 設計書

## 概要

Phase 7 は実行中タスクの**可視化・診断**機能を追加し、ローカル開発時のデバッグ体験を向上させる。

- **強化版 `lecs ps`** — ヘルス状態・ポート・起動時間 + `--output json/wide`
- **`lecs inspect <family>`** — 実行中タスクの詳細設定表示（secrets マスキング付き）
- **`lecs stats [family]`** — ライブリソース使用量（CPU%、メモリ、I/O）
- **`--events`** — 構造化ライフサイクルイベント（NDJSON 形式で stderr に出力）

対応要件: FR-11.1〜FR-11.4, FR-11.6

---

## アーキテクチャ

```
lecs ps [--output table|json|wide]
    │
    ▼
ContainerRuntime::list_containers()
    │
    ▼
format_table() / format_json()
    │  NAME, IMAGE, STATUS, HEALTH, PORTS, UPTIME, TASK
    ▼
stdout

lecs inspect <family>
    │
    ▼
list_containers() → filter by lecs.task label
    │
    ▼
inspect_container() per container
    │
    ▼
Display: ID, Image, Status, Health, Ports, Network, Environment
    │  (secrets masked via lecs.secrets label → ******)
    ▼
stdout

lecs stats [family]
    │
    ▼
list_containers() → stats_container() per container
    │
    ▼
format_stats_table()
    │  NAME, CPU%, MEM USAGE/LIMIT, NET I/O, BLOCK I/O
    ▼
stdout (single snapshot)

lecs run -f task-def.json --events
    │
    ▼
EventSink (NdjsonEventSink / NullEventSink)
    │  emit() at: Created, Started, HealthCheckPassed/Failed, Exited, CleanupCompleted
    ▼
stderr (NDJSON)
```

---

## モジュール配置

| ファイル | 責務 |
|---------|------|
| `src/cli/ps.rs` | `lecs ps` コマンド（テーブル拡張 + JSON/Wide 出力） |
| `src/cli/inspect.rs` | `lecs inspect` コマンド（実効設定表示 + secrets マスキング） |
| `src/cli/stats.rs` | `lecs stats` コマンド（リソース使用量表示） |
| `src/cli/mod.rs` | `InspectArgs`, `StatsArgs`, `OutputFormat` 型定義、`Command` enum 拡張、`RunArgs` に `--events` 追加 |
| `src/events/mod.rs` | 構造化イベントログ (`EventSink` trait + NDJSON) |
| `src/container/mod.rs` | `ContainerInfo` 拡張、`ContainerStats`、`PortInfo`、`stats_container()` |
| `src/orchestrator/mod.rs` | `orchestrate_startup()` にイベント発行追加 |
| `src/cli/run.rs` | `--events` 分岐、`lecs.secrets`/`lecs.depends_on` ラベル追加 |
| `src/main.rs` | `Inspect`, `Stats` ディスパッチ追加 |

---

## 型定義

### コンテナ層（`src/container/mod.rs`）

```rust
#[derive(Debug, Clone)]
pub struct PortInfo {
    pub host_port: Option<u16>,
    pub container_port: u16,
    pub protocol: String,
}

#[derive(Debug, Clone)]
pub struct ContainerStats {
    pub cpu_percent: f64,
    pub memory_usage: u64,
    pub memory_limit: u64,
    pub net_rx_bytes: u64,
    pub net_tx_bytes: u64,
    pub block_read_bytes: u64,
    pub block_write_bytes: u64,
}

// ContainerInfo 拡張フィールド:
pub health_status: Option<String>,  // "healthy"/"unhealthy"/"starting"/None
pub ports: Vec<PortInfo>,
pub started_at: Option<String>,     // ISO 8601

// ContainerInspection 拡張フィールド:
pub image: String,
pub env: Vec<String>,
pub network_name: Option<String>,
pub ports: Vec<PortInfo>,
pub started_at: Option<String>,
pub labels: HashMap<String, String>,
```

### イベント層（`src/events/mod.rs`）

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    Created,
    Started,
    HealthCheckPassed,
    HealthCheckFailed,
    Exited,
    CleanupCompleted,
}

#[derive(Debug, Clone, Serialize)]
pub struct LifecycleEvent {
    pub timestamp: String,        // ISO 8601
    pub event_type: EventType,
    pub container_name: Option<String>,
    pub family: String,
    pub details: Option<String>,
}

pub trait EventSink: Send + Sync {
    fn emit(&self, event: &LifecycleEvent);
}
```

### オーケストレータ層（`src/orchestrator/mod.rs`）

```rust
/// イベント発行コンテキスト。event_sink と family をバンドルして引数を削減する。
pub struct EventContext<'a> {
    pub event_sink: &'a dyn EventSink,
    pub family: &'a str,
}
```

### CLI 層（`src/cli/mod.rs`）

```rust
#[derive(Clone, Default, clap::ValueEnum)]
pub enum OutputFormat {
    #[default] Table,
    Json,
    Wide,
}

pub struct InspectArgs { pub family: String }
pub struct StatsArgs { pub family: Option<String> }

// RunArgs に追加:
pub events: bool,
```

---

## 公開 API

### `src/container/mod.rs`

| 関数 | シグネチャ | 説明 |
|------|----------|------|
| `stats_container` | `async fn(&self, id: &str) -> Result<ContainerStats>` | bollard stats one-shot 取得 |

### `src/cli/ps.rs`

| 関数 | 説明 |
|------|------|
| `format_table(containers) -> String` | 7列テーブル（NAME, IMAGE, STATUS, HEALTH, PORTS, UPTIME, TASK） |
| `format_json(containers) -> String` | JSON 配列出力 |
| `format_uptime(started_at) -> String` | 経過時間計算 ("2m30s" 形式) |
| `format_ports(ports) -> String` | "8080->80/tcp" 形式 |
| `format_bytes(bytes) -> String` | "1.2 MiB" 形式 |

### `src/cli/inspect.rs`

| 関数 | 説明 |
|------|------|
| `execute(args, host) -> Result<()>` | ファイル I/O ラッパー |
| `execute_with_client(args, client) -> Result<()>` | テスト可能なコアロジック |
| `parse_secret_names(labels) -> HashSet<String>` | `lecs.secrets` ラベルから秘密名リスト取得 |
| `mask_env_var(env_str, secret_names) -> String` | 秘密値を `******` に置換 |
| `format_inspect_env(env, secret_names) -> Vec<String>` | 環境変数リスト全体のマスキング |

### `src/cli/stats.rs`

| 関数 | 説明 |
|------|------|
| `execute(args, host) -> Result<()>` | ファイル I/O ラッパー |
| `execute_with_client(args, client) -> Result<()>` | テスト可能なコアロジック |
| `format_stats_table(containers, stats_results) -> String` | テーブルフォーマット |

### `src/events/mod.rs`

| 型 | 説明 |
|------|------|
| `NdjsonEventSink` | stderr に NDJSON で書き出す実装 |
| `NullEventSink` | 何もしない実装（`--events` 未指定時） |
| `CollectingEventSink` | テスト用：イベントを `Vec` に収集 |

### `src/orchestrator/mod.rs`（変更）

| 関数 / 型 | 変更 |
|------|------|
| `EventContext` | 新規構造体。`event_sink: &dyn EventSink` と `family: &str` をバンドル |
| `orchestrate_startup` | `event_sink: &dyn EventSink` パラメータ追加。Created/Started イベントを発行 |
| `wait_for_condition` | `ctx: &EventContext` パラメータ追加。Exited イベントを発行（Complete/Success 条件時） |
| `wait_for_healthy` | `ctx: &EventContext` パラメータ追加。HealthCheckPassed/HealthCheckFailed イベントを発行 |
| `create_and_start_container` | 新規ヘルパー。コンテナ作成+起動+イベント発行を集約 |

### `src/cli/run.rs`（変更）

| 関数 | 変更 |
|------|------|
| `cleanup` | `event_sink: &dyn EventSink`, `family: &str` パラメータ追加。CleanupCompleted イベントを発行 |

---

## データフロー

### `lecs ps`

1. `list_containers()` — ラベルフィルタ付きで実行中コンテナ一覧取得
2. `ContainerInfo` の拡張フィールド（`health_status`, `ports`, `started_at`）を利用
3. `--output` に応じて `format_table()` / `format_json()` で出力

### `lecs inspect <family>`

1. `list_containers()` → `lecs.task` ラベルで family フィルタ
2. 各コンテナに `inspect_container()` → `ContainerInspection` 取得
3. `lecs.secrets` ラベルから秘密名リスト取得（`parse_secret_names()`）
4. 環境変数の秘密値を `******` に置換（`mask_env_var()`）
5. コンテナ詳細を stdout に表示

### `lecs stats`

1. `list_containers()` → オプショナル family フィルタ
2. 各コンテナに `stats_container()` → `ContainerStats` 取得（失敗時は N/A）
3. `format_stats_table()` でテーブル表示（単発スナップショット）

### `--events`

1. `--events` 指定時: `NdjsonEventSink` を構築
2. 未指定時: `NullEventSink` を構築
3. `orchestrate_startup()` に `&dyn EventSink` を渡す
4. 全 6 種のイベントを発行:
   - `Created` — コンテナ作成時（`create_and_start_container`）
   - `Started` — コンテナ起動時（`create_and_start_container`）
   - `HealthCheckPassed` — ヘルスチェック成功時（`wait_for_healthy`）
   - `HealthCheckFailed` — ヘルスチェック失敗/タイムアウト時（`wait_for_healthy`）
   - `Exited` — コンテナ終了時（`wait_for_condition` Complete/Success 条件）
   - `CleanupCompleted` — クリーンアップ完了時（`cleanup`）
5. NDJSON 形式で stderr に出力

---

## エラーハンドリング

| ケース | 挙動 | 型 |
|--------|------|---|
| stats 取得失敗 | 該当コンテナの列を "N/A" で表示 | `ContainerError` |
| inspect で family 不一致 | "No containers found" エラー | `anyhow::Error` |
| inspect のコンテナ inspect 失敗 | スキップ（該当コンテナのみ） | `ContainerError` |

---

## 技術選定

| 項目 | 選定 | 理由 |
|------|------|------|
| stats 取得 | bollard `StatsOptions { stream: false, one_shot: true }` | 既存依存。単発取得で十分 |
| CPU% 計算 | `(cpu_delta / system_delta) * num_cpus * 100.0` | Docker stats と同じアルゴリズム |
| 時刻パース | `chrono::DateTime::parse_from_rfc3339` | 既存依存 |
| 経過時間 | `chrono::Utc::now() - started_at` | 既存依存 |
| イベント出力 | stderr + NDJSON (`serde_json::to_string`) | 既存依存。stdout と分離 |
| secrets マスキング | `lecs.secrets` コンテナラベル | ラベル経由で inspect 時に秘密名を特定 |

---

## 既存コード再利用

| 再利用対象 | ファイル | 用途 |
|-----------|---------|------|
| `ContainerRuntime` trait | `src/container/mod.rs` | 全コマンドのコンテナ操作 |
| `MockContainerClient` | `src/container/mod.rs` (test_support) | 全テスト |
| `execute()` + `execute_with_client()` パターン | `src/cli/ps.rs`, `src/cli/logs.rs` | 新コマンドの構造 |
| `format_table()` パターン | `src/cli/ps.rs` | テーブル表示 |
| `build_container_config()` | `src/cli/run.rs` | ラベル追加 |
| `chrono` クレート | `Cargo.toml` | 時刻パース・経過時間計算 |
| `serde_json` | `Cargo.toml` | JSON 出力・NDJSON |

---

## テスト

| テスト対象 | テスト数 |
|-----------|---------|
| `ContainerInfo` 拡張（ports, health, started_at） | 既存テスト更新 |
| `ContainerStats` + `stats_container()` | 2 |
| `PortInfo` + `calculate_cpu_percent()` | 2 |
| `cli/ps.rs` テーブル拡張 + JSON 出力 | 15 |
| `cli/inspect.rs` | 8 |
| `cli/stats.rs` | 5 |
| `events/mod.rs` | 7 |
| `cli/mod.rs` CLI パース | 10 |
| `orchestrator/mod.rs` イベント統合 | 既存テスト更新 |
| **合計** | **~49** |
