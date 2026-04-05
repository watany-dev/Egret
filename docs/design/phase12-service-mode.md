# Phase 12: サービスモード MVP — 実装計画

## Context

Lecs は Phase 0-11 を完了し、ECS タスクランナーとしての主要機能を全て実装済み（約15,500行、29ファイル、95%+カバレッジ）。次のステップは Phase 12「サービスモード MVP」で、ECS Service の最小要件をローカルで実現する。

**対応要件**: FR-17.1〜FR-17.4（`docs/requirements.md`）
**設計根拠**: `docs/design/service-gap-analysis.md` の MVP パス（Gap 1 + Gap 4 + Gap 6）

**解決する課題**: 現在の Lecs はワンショット実行のみ。コンテナが crash しても再起動されず、長時間稼働ではクレデンシャルが期限切れになる。開発者が「サービスのように」タスクを常時稼働させたい場合、手動で再起動する必要がある。

**成果**: `lecs run --service` で、コンテナ障害時の自動再起動・長時間稼働・クレデンシャル自動更新が可能になる。

**設計判断: Docker restart policy を使わない理由**:
Docker 自体の `--restart` ポリシーはコンテナ単位で機能するが、Lecs はタスク全体の `dependsOn` DAG やメタデータサーバーの `container_id` 更新を制御する必要がある。Docker に再起動を委ねるとこれらの整合性が保てないため、Lecs 自身がリスタートを管理する。

---

## サービスモードループのデータフロー

```
lecs run --service -f task-def.json
    │
    ▼
run_task() — 初回起動（DAG解決 → コンテナ起動）
    │
    ▼
┌──────────── service loop ◄────────────────────┐
│                                                │
├── tokio::select! {                             │
│     essential_exit = watch_essential_exit()     │
│     signal = ctrl_c()                          │
│     _ = log_streaming()                        │
│   }                                            │
│                                                │
├── essential コンテナ終了:                      │
│     RestartTracker::should_restart(exit_code)?  │
│     ├── true:                                  │
│     │   emit(Restarting)                        │
│     │   backoff wait (1s→2s→...→300s)          │
│     │   stop_container → remove_container      │
│     │   create_container → start_container     │
│     │   update_container_id (metadata)         │
│     │   restart log stream for container       │
│     │   └──────────────────────────────────────┘
│     └── false:
│         emit(MaxRestartsExceeded) or exit
│         → cleanup → 終了
│
└── Ctrl+C:
    → cleanup → 終了
```

---

## 実装順序（3イテレーション）

### イテレーション 1: リスタートポリシーの型定義と基盤

**目的**: `RestartPolicy` の型とバックオフロジックを定義し、テストを書く
**対応要件**: FR-17.1, FR-17.2

#### 変更ファイル
- `src/orchestrator/mod.rs` — `RestartPolicy` enum + `RestartTracker` 構造体を追加
- `src/events/mod.rs` — `Restarting` / `MaxRestartsExceeded` イベントタイプを追加

#### 詳細

1. **`RestartPolicy` enum** を `src/orchestrator/mod.rs` に追加:
```rust
/// Container restart policy for service mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    /// Do not restart (default, task-runner behavior).
    #[default]
    None,
    /// Restart only on non-zero exit code.
    OnFailure,
    /// Always restart regardless of exit code.
    Always,
}
```

2. **`RestartTracker` 構造体** を追加（**コンテナごと**に1インスタンス）:
```rust
/// Tracks restart state for a single container.
#[derive(Debug)]
pub struct RestartTracker {
    policy: RestartPolicy,
    restart_count: u32,
    max_restarts: u32,  // ローカル安全装置（デフォルト: 10）
}

/// Maximum backoff duration (5 minutes).
const MAX_BACKOFF_SECS: u64 = 300;
/// Default maximum restart attempts before giving up.
const DEFAULT_MAX_RESTARTS: u32 = 10;
```
- `new(policy, max_restarts) -> Self`
- `should_restart(&self, exit_code: i64) -> bool` — ポリシーに基づく判定 + max_restarts チェック
- `next_backoff(&self) -> Duration` — `min(2^restart_count, MAX_BACKOFF_SECS)` 秒
- `record_restart(&mut self)` — カウンタ増加
- `reset(&mut self)` — コンテナが安定稼働後のカウンタリセット

3. **`OrchestratorError::MaxRestartsExceeded`** を追加:
```rust
#[error("container '{0}' exceeded maximum restart count ({1})")]
MaxRestartsExceeded(String, u32),
```

4. **`EventType` に2バリアント追加** (`src/events/mod.rs`):
```rust
/// Container is being restarted.
Restarting,
/// Container exceeded maximum restart count.
MaxRestartsExceeded,
```

5. **テスト** (10-12テスト):
   - `should_restart`: `None` + exit 0/1 → `false`
   - `should_restart`: `OnFailure` + exit 0 → `false`、exit 1 → `true`
   - `should_restart`: `Always` + exit 0/1 → `true`
   - `should_restart`: max_restarts 超過 → `false`（全ポリシー）
   - `next_backoff`: count 0→1s, 1→2s, 2→4s, ..., 8→256s, 9→300s(cap)
   - `record_restart` + `reset`: カウンタ動作
   - `RestartPolicy::Default` → `None`
   - `EventType::Restarting` / `MaxRestartsExceeded` のシリアライゼーション

#### 再利用する既存コード
- `OrchestratorError` enum（`src/orchestrator/mod.rs:11`）— `MaxRestartsExceeded` バリアント追加
- `EventType` / `LifecycleEvent`（`src/events/mod.rs`）— 2バリアント追加

---

### イテレーション 2: `--service` フラグとサービスモードループ

**目的**: CLI に `--service` フラグを追加し、サービスモードのメインループを実装
**対応要件**: FR-17.3

#### 変更ファイル
- `src/cli/mod.rs` — `RunArgs` に `--service` フラグ追加
- `src/cli/task_lifecycle.rs` — `run_service_loop()` 関数を追加
- `src/cli/run.rs` — `execute()` でサービスモード分岐

#### 詳細

1. **`RunArgs` に `--service` フラグ追加** (`src/cli/mod.rs:283`):
```rust
/// Run in service mode (auto-restart containers, long-running until Ctrl+C)
#[arg(long, conflicts_with = "dry_run")]
pub service: bool,
```
- `--service` と `--dry-run` は排他（`conflicts_with`）
- `--service` と `watch` は独立コマンドのため競合しない

2. **`run_service_loop()` を `src/cli/task_lifecycle.rs` に追加**:

```rust
/// Run task in service mode with auto-restart.
///
/// Monitors essential containers and restarts them on failure
/// according to the restart policy. Runs until Ctrl+C or max
/// restarts exceeded.
#[cfg(not(tarpaulin_include))]
pub async fn run_service_loop(
    client: &(impl ContainerRuntime + ?Sized),
    task_def: &TaskDefinition,
    metadata_port: Option<u16>,
    auth_token: Option<&str>,
    metadata_state: Option<&SharedState>,
    event_sink: &dyn EventSink,
) -> Result<()> { ... }
```

   - `HashMap<String, RestartTracker>` でコンテナごとのリスタート状態を管理
   - 初回: `run_task()` で全コンテナ起動
   - ループ:
     - `tokio::select!` で essential コンテナの `wait_container` と `ctrl_c()` を並行監視
     - essential コンテナ終了時:
       - `tracker.should_restart(exit_code)` で判定
       - `true` → `tracker.record_restart()` → `tracker.next_backoff()` 待機 → 単一コンテナ再作成
       - `false` → `MaxRestartsExceeded` エラー or 正常終了 → 全体クリーンアップ
     - 非essential コンテナ終了: ログ出力のみ、再起動なし
     - Ctrl+C → 全体クリーンアップ

3. **単一コンテナ再起動関数**:
```rust
/// Restart a single container within a running task.
async fn restart_container(
    client: &(impl ContainerRuntime + ?Sized),
    container_name: &str,
    old_id: &str,
    config: &ContainerConfig,
    metadata_state: Option<&SharedState>,
    event_sink: &dyn EventSink,
    family: &str,
) -> Result<String> { ... }
```
   - `stop_container(old_id)` → `remove_container(old_id)` → `create_container(config)` → `start_container(new_id)`
   - `metadata::update_container_id(state, name, new_id)` で ID 更新
   - 再起動失敗時: エラーログ出力、次のバックオフで再試行（再起動失敗もリスタートカウントに加算）

4. **ログストリーミングの設計**:
   - ワンショットモード: 既存の `stream_logs_until_signal()` をそのまま使用
   - サービスモード: ログストリーミングはコンテナ再起動時に古い `JoinHandle` を `abort()` し、新しいコンテナの `stream_logs` を `tokio::spawn` で開始。`HashMap<String, JoinHandle>` で管理

5. **`execute()` の分岐** (`src/cli/run.rs:18`):
```rust
if args.service {
    run_service_loop(client, task_def, metadata_port, auth_token, metadata_state, event_sink).await
} else {
    // 既存のワンショット実行（変更なし）
}
```

6. **`--dry-run` のサービスモード表示**:
   - `--service` 指定時、dry-run 出力にリスタートポリシー情報を追加
   - ただし `conflicts_with = "dry_run"` のため実際には同時指定不可

7. **テスト** (5-8テスト):
   - CLI パース: `--service` の有無
   - CLI パース: `--service --dry-run` が排他エラー
   - `restart_container` のモックテスト（`MockContainerClient` 使用）
   - サービスループの終了条件テスト（max_restarts 到達）

#### 再利用する既存コード
- `run_task()` (`src/cli/task_lifecycle.rs:25`) — 初回起動
- `cleanup()` (`src/cli/task_lifecycle.rs:126`) — 最終クリーンアップ
- `build_container_config()` (`src/cli/task_lifecycle.rs:187`) — 再起動時のコンテナ設定構築
- `watch_essential_exit()` (`src/orchestrator/mod.rs:418`) — essential コンテナ監視
- `metadata::update_container_id()` (`src/metadata/mod.rs:264`) — 再起動後の ID 更新

---

### イテレーション 3: クレデンシャルローテーション

**目的**: バックグラウンドでクレデンシャルを定期リフレッシュ
**対応要件**: FR-17.4

#### 変更ファイル
- `src/credentials/mod.rs` — `CredentialRefresher` 構造体 + `compute_refresh_interval()` を追加
- `src/cli/task_lifecycle.rs` — サービスモード時にリフレッシュタスクを起動

#### 詳細

1. **`compute_refresh_interval()`** を `src/credentials/mod.rs` に追加（テスト可能な純粋関数）:
```rust
/// Default refresh interval when TTL cannot be determined (30 minutes).
const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// Minimum refresh interval to avoid excessive API calls (1 minute).
const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// Compute the credential refresh interval from an expiration timestamp.
///
/// Returns `min(TTL / 2, 30 minutes)`, clamped to at least 1 minute.
pub fn compute_refresh_interval(expiration: &str) -> Duration { ... }
```

2. **`CredentialRefresher`** を `src/credentials/mod.rs` に追加:
```rust
/// Background credential refresher for long-running service mode.
pub struct CredentialRefresher {
    state: SharedState,   // Arc<RwLock<ServerState>>
    role_arn: Option<String>,
}
```
- `new(state, role_arn) -> Self`
- `start(self) -> JoinHandle<()>` — `tokio::spawn` でバックグラウンドリフレッシュ開始
- リフレッシュループ:
  1. `compute_refresh_interval()` で現在のクレデンシャルから間隔を算出
  2. `tokio::time::sleep(interval)` で待機
  3. `load_local_credentials(role_arn)` で再取得
  4. 成功 → `state.write().credentials = Some(new_creds)`
  5. 失敗 → `tracing::warn!`、既存クレデンシャルを維持、60秒後にリトライ

3. **`ServerState`** は既に `Arc<RwLock<ServerState>>` で保護済み（`src/metadata/mod.rs:209`）
   - 既存の `credentials: Option<AwsCredentials>` フィールドをそのまま上書き更新
   - ロック競合は最小（読み取りは HTTP リクエスト時のみ、書き込みは数十分に1回）

4. **サービスモード時の起動** (`src/cli/task_lifecycle.rs`):
   - `--service` かつ `--no-metadata` でない場合のみ `CredentialRefresher::start()` を呼ぶ
   - `run_service_loop()` 内で `JoinHandle` を保持
   - シャットダウン時に `handle.abort()` で停止

5. **テスト** (4-6テスト):
   - `compute_refresh_interval`: 有効な ISO8601 → 正しい Duration
   - `compute_refresh_interval`: 遠い未来の期限 → 30分上限
   - `compute_refresh_interval`: 既に期限切れ → 最小1分
   - `compute_refresh_interval`: パース不能な文字列 → デフォルト30分
   - `CredentialRefresher::new()` の構築テスト

#### 再利用する既存コード
- `load_local_credentials()` (`src/credentials/mod.rs:65`) — リフレッシュ時の呼び出し
- `SharedState` / `ServerState` (`src/metadata/mod.rs:34`) — credentials フィールドの書き換え
- `AwsCredentials.expiration` (`src/credentials/mod.rs:38`) — TTL 計算の入力

---

## 設計書の更新

実装完了後、以下のドキュメントを更新:

### `docs/design/phase12-service-mode.md` — 新規設計書

標準構成に従う:
1. 概要（対応要件 FR-17.1〜FR-17.4 を明記）
2. アーキテクチャ（サービスモードループのデータフロー図）
3. 型定義（`RestartPolicy`, `RestartTracker`, `CredentialRefresher`）
4. エラー型（`OrchestratorError::MaxRestartsExceeded`）
5. 公開 API（`run_service_loop`, `restart_container`, `compute_refresh_interval`）
6. テスト戦略
7. イテレーション履歴

### `docs/ROADMAP.md` — Phase 12 のチェックボックスを完了に
### `docs/requirements.md` — FR-17.1〜FR-17.4 のステータスを ✅ に更新

---

## 検証方法

### ユニットテスト
```bash
make test
```
- `RestartPolicy` / `RestartTracker` の全パターン（10-12テスト）
- `compute_refresh_interval` の全パターン（4-6テスト）
- CLI パース（`--service` フラグ、排他制約）
- `EventType::Restarting` / `MaxRestartsExceeded` のシリアライゼーション
- `restart_container` のモックテスト

### 品質チェック
```bash
make check  # fmt-check + lint + test + doc + deny
```

### カバレッジ
```bash
make coverage  # 95% 以上を維持
```
- `run_service_loop` は `#[cfg(not(tarpaulin_include))]`（コンテナランタイム依存）
- `CredentialRefresher::start()` の実リフレッシュも同様
- テスト可能なロジック（`RestartTracker`, `compute_refresh_interval`, `restart_container`）は通常のユニットテストでカバー

### 手動テスト（コンテナランタイムがある環境）
1. `lecs run -f tests/fixtures/simple-task.json --service` — 常時稼働を確認
2. コンテナを手動停止 (`docker stop`) → 自動再起動を確認
3. Ctrl+C → グレースフルシャットダウンを確認
4. 10回連続 crash → `MaxRestartsExceeded` でタスク終了を確認
5. 長時間稼働（1h+）→ クレデンシャルが更新されることをログで確認

---

## update-plan 検証結果

### 設計書品質評価

対象設計書: `docs/design/service-gap-analysis.md`（Phase 12 専用設計書は未作成 → イテレーション完了後に作成予定）

| 設計書 | モジュール設計 | API互換 | エラー処理 | 技術選定 | データフロー | 平均 |
|--------|-------------|---------|-----------|---------|------------|------|
| service-gap-analysis.md | 80/100 | 75/100 | 70/100 | 80/100 | 75/100 | 76.0 |
| プラン（改善後） | 90/100 | 85/100 | 85/100 | 90/100 | 90/100 | 88.0 |

**総合判定**: 🟡 軽微な改善後に着手可能（平均 88.0 → 90 に近い水準）

### 整合性チェック

| チェック項目 | スコア | 詳細 |
|-------------|--------|------|
| 設計書 ↔ ソースコード | 90/100 | gap-analysis の既存コード記述は正確。`watch_essential_exit`, `ServerState`, `ContainerRuntime` トレイトの再利用性分析が的確 |
| ロードマップ ↔ 設計書 ↔ 要件定義 | 95/100 | ROADMAP Phase 12 の3項目 ↔ FR-17.1〜FR-17.4 ↔ gap-analysis MVP パスが完全に対応 |

### 修正事項（プラン改善で反映済み）

- **P1-1**: `RestartTracker` のスコープを明確化 — コンテナごとに1インスタンス（`HashMap<String, RestartTracker>`）を明記
- **P1-2**: 再起動失敗時のエラーハンドリングを追加 — 再起動失敗もカウントに加算、次のバックオフで再試行
- **P1-3**: `OrchestratorError::MaxRestartsExceeded` バリアントを追加 — 明確なエラー報告
- **P1-4**: ログストリーミングの再起動対応を明記 — コンテナ再起動時に新しいストリームを接続
- **P1-5**: `--service` と `--dry-run` の排他制約を追加（`conflicts_with`）
- **P1-6**: Docker restart policy を使わない設計判断の根拠を明記
- **P1-7**: サービスモードループのデータフロー図を追加
- **P2-1**: `compute_refresh_interval()` を純粋関数として分離 — テスタビリティ向上
- **P2-2**: `EventType::MaxRestartsExceeded` を追加 — 可観測性の向上

---

## イテレーション履歴

実装は TDD の Red → Green → Refactor サイクルで 3 イテレーションに分割:

### Iteration 1: リスタートポリシーの型定義と基盤
- `src/events/mod.rs`: `EventType::Restarting` / `MaxRestartsExceeded` バリアントを追加
- `src/orchestrator/mod.rs`: `RestartPolicy` enum、`RestartTracker` 構造体（`new`, `should_restart`, `next_backoff`, `record_restart`, `reset` を `const fn` で実装）、`OrchestratorError::MaxRestartsExceeded` バリアントを追加
- 定数: `MAX_BACKOFF_SECS = 300`、`DEFAULT_MAX_RESTARTS = 10`
- テスト: `RestartTracker` / `RestartPolicy` のパターン網羅 13 テスト + イベントシリアライズ 2 テスト

### Iteration 2: `--service` フラグとサービスモードループ
- `src/cli/mod.rs`: `RunArgs` に `--service` / `--restart` / `--max-restarts` フラグ、`RestartPolicyArg` enum、`From<RestartPolicyArg> for RestartPolicy` 実装（`conflicts_with = "dry_run"` / `requires = "service"` の clap 制約を付与）
- `src/cli/task_lifecycle.rs`: `restart_container` 関数、`RestartOutcome` enum（`Replaced` / `RemovedButNotCreated` / `FailedBeforeRemoval` の3状態）を追加
- `src/cli/run.rs`: `run_service_loop` 実装（essential コンテナ watcher を `tokio::mpsc` で集約し、`tokio::select!` で Ctrl+C との競合待ち）、`attempt_restart` ヘルパー、`spawn_essential_watcher` / `spawn_log_stream` を抽出
- テスト: `restart_container` のモックテスト 6 件、CLI パーステスト 5 件

### Iteration 3: クレデンシャルローテーション
- `src/credentials/mod.rs`: `compute_refresh_interval` 純粋関数、`CredentialRefresher` 構造体（`new`, `update_state`, `start` を提供、`start` は `#[cfg(not(tarpaulin_include))]` でカバレッジ除外）
- 定数: `DEFAULT_REFRESH_INTERVAL = 30min`、`MIN_REFRESH_INTERVAL = 1min`、`MAX_REFRESH_INTERVAL = 30min`
- 更新間隔: `min(TTL/2, MAX_REFRESH_INTERVAL)` を下限 `MIN_REFRESH_INTERVAL` でクランプ、失敗時は 60s 後にリトライ
- `src/cli/run.rs`: `run_service_loop` に `CredentialRefresher` を統合（`metadata_state: Some(_)` のときのみ起動、シャットダウン時に `handle.abort()` を watcher/log よりも先に実行）
- テスト: `compute_refresh_interval` 6 件、`CredentialRefresher::new` / `update_state` 4 件
