# Phase 11: ECS Exec + 環境変数拡張 設計書

## 概要

Phase 11 は**デバッグ体験の向上**を目標とし、以下の機能を追加する。

- **`lecs exec`** — Lecs 管理コンテナ内でコマンドを実行（`aws ecs execute-command` 相当）
- **`environmentFiles`** — ローカル .env ファイルから環境変数を読み込み `environment` にマージ
- **`ulimits`** — コンテナのリソースリミット（nofile, memlock 等）を設定
- **`linuxParameters`** — init プロセス、共有メモリサイズ、tmpfs マウントを設定

対応要件: FR-16.1〜FR-16.4

---

## アーキテクチャ

### `lecs exec` データフロー

```
lecs exec <container> [-- command...]
    │
    ▼
ContainerRuntime::list_containers()
    │
    ▼
format::find_container(containers, query)
    │  exact match → partial match → NotFound / Ambiguous
    ▼
ContainerRuntime::exec_container(id, cmd)
    │
    ├── bollard::create_exec(CreateExecOptions { attach_stdout, attach_stderr, cmd })
    │       → CreateExecResults { id: exec_id }
    │
    ├── bollard::start_exec(exec_id)
    │       → StartExecResults::Attached { output }
    │       → LogOutput::StdOut → stdout
    │       → LogOutput::StdErr → stderr
    │
    └── bollard::inspect_exec(exec_id)
            → ExecInspectResponse { exit_code }
            → non-zero → process::exit(code)
```

### `environmentFiles` データフロー

```
task-def.json
    │
    ├── containerDefinitions[].environmentFiles[].value
    │       → ローカルファイルパスとして解釈
    │       → .env 形式でパース (KEY=VALUE, # コメント, 空行スキップ)
    │
    └── containerDefinitions[].environment[]
            → environmentFiles の変数を先にロード
            → task-def の environment が後で上書き（environment 優先）

マージ順序:
  environmentFiles[0].env → environmentFiles[1].env → ... → container.environment
  (後から追加されたものが同名キーを上書き)
```

### bollard 設定マッピング

```
タスク定義フィールド              → bollard HostConfig フィールド
─────────────────────────────────────────────────────────────
ulimits[].name/softLimit/hardLimit → HostConfig::ulimits (ResourcesUlimits)
linuxParameters.initProcessEnabled → HostConfig::init
linuxParameters.sharedMemorySize   → HostConfig::shm_size (MiB → bytes)
linuxParameters.tmpfs[]            → HostConfig::tmpfs (HashMap<String, String>)
```

---

## モジュール配置

| ファイル | 責務 |
|---------|------|
| `src/cli/exec.rs` | exec コマンド実装 |
| `src/cli/mod.rs` | `Exec(ExecArgs)` バリアント + `ExecArgs` 定義 |
| `src/cli/format.rs` | `find_container` / `FindContainerError` 共通ユーティリティ |
| `src/cli/run.rs` | `load_environment_files` + `build_container_config` 拡張 + dry-run 表示 |
| `src/taskdef/mod.rs` | `EnvironmentFile`, `Ulimit`, `LinuxParameters`, `TmpfsMount` 型定義 |
| `src/container/mod.rs` | `exec_container` トレイトメソッド + `UlimitConfig`, `ExecResult` 型 |
| `src/main.rs` | `Command::Exec` マッチアーム |

---

## 型定義

### タスク定義型（`src/taskdef/mod.rs`）

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentFile {
    pub value: String,
    #[serde(default = "default_env_file_type")]
    pub r#type: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ulimit {
    pub name: String,
    pub soft_limit: i64,
    pub hard_limit: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinuxParameters {
    #[serde(default)]
    pub init_process_enabled: Option<bool>,
    #[serde(default)]
    pub shared_memory_size: Option<i64>,
    #[serde(default)]
    pub tmpfs: Vec<TmpfsMount>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TmpfsMount {
    pub container_path: String,
    pub size: i64,
    #[serde(default)]
    pub mount_options: Vec<String>,
}
```

### コンテナランタイム型（`src/container/mod.rs`）

```rust
pub struct UlimitConfig {
    pub name: String,
    pub soft: i64,
    pub hard: i64,
}

pub struct ExecResult {
    pub exit_code: Option<i64>,
}
```

---

## エラー型

### `TaskDefError` 追加バリアント

```rust
#[error("failed to read environment file {path}: {source}")]
EnvironmentFileRead { path: PathBuf, source: std::io::Error },

#[error("invalid line in environment file {path} at line {line_number}: {detail}")]
EnvironmentFileParse { path: PathBuf, line_number: usize, detail: String },
```

### `ContainerError` 追加バリアント

```rust
#[error("exec failed on container {container_id}: {detail}")]
ExecFailed { container_id: String, detail: String },
```

---

## 公開 API

### `ContainerRuntime` トレイト拡張

```rust
async fn exec_container(
    &self,
    id: &str,
    cmd: &[String],
) -> Result<ExecResult, ContainerError>;
```

### CLI コマンド

```
lecs exec <container> [-- <command>...]
```

- `container`: コンテナ名（完全一致 → 部分一致で解決）
- `command`: 実行コマンド（省略時 `/bin/sh`）
- `--` 区切りはオプショナル

### `load_environment_files` 関数

```rust
fn load_environment_files(
    files: &[EnvironmentFile],
) -> Result<Vec<KeyValuePair>>;
```

- .env 形式パース: `KEY=VALUE`、`#` コメント、空行スキップ
- 引用符（`"` / `'`）の自動除去
- `=` を含まない行はスキップ（エラーにしない）

---

## テスト戦略

| テスト対象 | テスト数 | カテゴリ |
|-----------|---------|---------|
| `find_container` 共通化 | 5 | ユニット |
| `EnvironmentFile` デシリアライズ | 3 | ユニット |
| `load_environment_files` パース | 7 | ユニット |
| `Ulimit` デシリアライズ + bollard 変換 | 3 | ユニット |
| `LinuxParameters` デシリアライズ + bollard 変換 | 4 | ユニット |
| `ExecArgs` パース | 2 | ユニット |
| dry-run 新フィールド表示 | 1 | ユニット |

合計: ~25 テスト追加（478 → 502）

---

## イテレーション

| # | 内容 | コミット |
|---|------|---------|
| 0-a | FR-15 ドキュメントステータス更新 | `docs: update FR-15 status` |
| 0-b | `find_container` を `format.rs` に抽出（Tidy First?） | `tidy: extract find_container` |
| 1 | `environmentFiles` パース + マージ + テスト | `feat: add environmentFiles support` |
| 2 | `ulimits` フィールド + bollard マッピング + テスト | `feat: add ulimits support` |
| 3 | `linuxParameters` フィールド + bollard マッピング + テスト | `feat: add linuxParameters support` |
| 4 | `lecs exec` コマンド + `ContainerRuntime::exec_container` | `feat: add lecs exec command` |
| 5 | dry-run / validate での新フィールド表示 | `feat: add dry-run display for new fields` |
| 6 | ドキュメント更新（requirements.md, ROADMAP.md, 設計書） | `docs: update Phase 11 documentation` |
