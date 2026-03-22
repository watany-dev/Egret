# dependsOn + Health Check 設計書（Phase 4）

## 概要

Phase 4 では ECS の `dependsOn` DAG 解決とヘルスチェック機能を実装し、マルチコンテナタスクの起動順序と健全性を制御する。

**対象要件**: FR-8.1〜FR-8.5

## モジュール配置

| ファイル | 責務 |
|---------|------|
| `src/taskdef/mod.rs` | `DependsOn`, `HealthCheck`, `DependencyCondition` 型定義 + バリデーション |
| `src/container/mod.rs` | `HealthCheckConfig`, `ContainerInspection`, `ContainerState`, `WaitResult` 型追加。`ContainerRuntime` トレイト拡張（`inspect_container`, `wait_container`）|
| `src/orchestrator/mod.rs` | DAG 解決（Kahn's アルゴリズム）、条件待機、ヘルスチェックポーリング、essential 監視 |
| `src/cli/run.rs` | `TaskDefinition` → `Vec<ContainerSpec>` 変換、`orchestrate_startup()` 呼び出し |

## 型定義

### taskdef 層（ECS 互換、秒単位）

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DependencyCondition { Start, Complete, Success, Healthy }

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DependsOn {
    pub container_name: String,
    pub condition: DependencyCondition,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCheck {
    pub command: Vec<String>,
    #[serde(default = "default_health_interval")]
    pub interval: u32,     // default 30
    #[serde(default = "default_health_timeout")]
    pub timeout: u32,      // default 5
    #[serde(default = "default_health_retries")]
    pub retries: u32,      // default 3
    #[serde(default)]
    pub start_period: u32, // default 0
}
```

### container 層

```rust
pub struct HealthCheckConfig {
    pub test: Vec<String>,
    pub interval_ns: i64,
    pub timeout_ns: i64,
    pub retries: i64,
    pub start_period_ns: i64,
}

pub struct ContainerInspection {
    pub id: String,
    pub state: ContainerState,
}

pub struct ContainerState {
    pub status: String,
    pub running: bool,
    pub exit_code: Option<i64>,
    pub health_status: Option<String>,
}

pub struct WaitResult {
    pub status_code: i64,
}
```

### orchestrator 層

```rust
/// イベント発行コンテキスト（Phase 7 で追加）
pub struct EventContext<'a> {
    pub event_sink: &'a dyn EventSink,
    pub family: &'a str,
}

pub struct DependencyInfo {
    pub name: String,
    pub depends_on: Vec<DependsOn>,
}

pub struct ContainerSpec {
    pub name: String,
    pub config: ContainerConfig,
    pub depends_on: Vec<DependsOn>,
    pub health_check: Option<HealthCheck>,
    pub essential: bool,
}

pub struct StartupResult {
    pub started: Vec<(String, String)>,
}

pub struct EssentialExit {
    pub container_name: String,
    pub exit_code: i64,
}
```

## エラー型

```rust
#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    #[error("cyclic dependency detected: {0}")]
    CyclicDependency(String),

    #[error("container runtime error: {0}")]
    Runtime(#[from] ContainerError),

    #[error("condition not met for container '{0}': {1}")]
    ConditionNotMet(String, String),

    #[error("essential container '{0}' exited with code {1}")]
    EssentialContainerFailed(String, i64),

    #[error("health check timed out for container '{0}'")]
    HealthCheckTimeout(String),
}
```

## 公開 API

### DAG 解決

```rust
pub fn resolve_start_order(deps: &[DependencyInfo]) -> Result<Vec<Vec<String>>, OrchestratorError>
```

Kahn's アルゴリズムでレイヤー化されたトポロジカル順序を返す。循環検出時は DFS で循環パスを報告。

### コンテナ起動オーケストレーション

```rust
pub async fn orchestrate_startup(
    client: &(impl ContainerRuntime + ?Sized),
    specs: Vec<ContainerSpec>,
    event_sink: &dyn EventSink,
) -> Result<StartupResult, (StartupResult, OrchestratorError)>
```

エラー時も `StartupResult` を返し、呼び出し元がクリーンアップ可能にする。
`event_sink` を通じてライフサイクルイベント（Created, Started 等）を発行する（Phase 7 で追加）。

### 条件待機

```rust
pub async fn wait_for_condition(
    client: &(impl ContainerRuntime + ?Sized),
    id: &str, name: &str,
    condition: DependencyCondition,
    health_check: Option<&HealthCheck>,
    ctx: &EventContext<'_>,
) -> Result<(), OrchestratorError>
```

`EventContext` を通じて Exited / HealthCheckPassed / HealthCheckFailed イベントを発行する（Phase 7 で追加）。

### Essential 監視

```rust
pub async fn watch_essential_exit(
    client: &(impl ContainerRuntime + ?Sized),
    id: &str, name: &str,
) -> EssentialExit
```

## データフロー

```
TaskDefinition
    │
    ▼ (cli/run.rs: build specs)
Vec<ContainerSpec>
    │
    ▼ (orchestrator: resolve_start_order)
Vec<Vec<String>> = [[db], [app, migration]]
    │                  ↑Layer 0   ↑Layer 1
    ▼
┌─────────────── Layer 0 ───────────────┐
│  create_container(db) → start(db)     │
└───────────────────────────────────────┘
    │
    ▼ 条件待機: wait_for_condition(db, HEALTHY)
    │
┌─────────────── Layer 1 ───────────────┐
│  create_container(app)  → start(app)  │
│  create_container(migr) → start(migr) │
└───────────────────────────────────────┘
    │
    ▼
Ok(StartupResult { started: [...] })
```

### ECS 起動条件セマンティクス

| 条件 | 判定方法 |
|------|---------|
| `START` | `start_container()` 成功で即完了 |
| `COMPLETE` | `wait_container()` で終了待ち |
| `SUCCESS` | `wait_container()` → exit_code == 0 検証 |
| `HEALTHY` | `inspect_container()` ポーリングで `health_status == "healthy"` 確認 |

### ヘルスチェックタイムアウト計算

```
timeout = start_period + interval * (retries + 1) + 30秒バッファ
```

## 技術選定

### DAG アルゴリズム: Kahn's (BFS)

レイヤー（並行起動可能グループ）を自然に出力。循環検出が副産物として得られる。`petgraph` は不要（コンテナ数は通常 2〜10 個）。

### ヘルスチェック監視: ポーリング

`inspect_container` をポーリング。Docker Events API は Podman での挙動差異があり、テストも困難。ポーリング間隔は `health_check.interval` と一致（ECS の挙動と同一）。

### 秒→ナノ秒変換

taskdef は ECS 互換の秒単位、bollard はナノ秒。変換は `build_container_config` で `i64::from(seconds) * 1_000_000_000`。

## バリデーション

`TaskDefinition::validate_depends_on()` で以下を検出:
1. 自己参照
2. 存在しないコンテナ名への参照
3. `HEALTHY` 条件の対象に `healthCheck` がない

## テスト戦略

| テスト対象 | テスト数 | 方法 |
|-----------|---------|------|
| taskdef パース（dependsOn, healthCheck） | 6 | JSON パース + フィールド検証 |
| taskdef バリデーション | 4 | エラーケース検証 |
| DAG 解決（トポロジカルソート） | 9 | 純粋関数テスト |
| 条件待機 | 5 | `MockContainerClient` + `tokio::time::pause` |
| Essential 監視 | 2 | `MockContainerClient` |
| `orchestrate_startup` | 4 | 統合テスト |
| `build_container_config` (healthCheck) | 1 | 秒→ナノ秒変換検証 |
| `run_task` (依存関係付き) | 1 | 起動順序検証 |
| **合計** | **32** | |
