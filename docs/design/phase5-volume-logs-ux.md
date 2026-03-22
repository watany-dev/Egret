# Phase 5: Volume + Logs + UX 改善 — 設計書

## Context

Phase 0-4 で Lecs はタスク定義パース、コンテナ実行、オーバーライド、シークレット、メタデータ/クレデンシャル、dependsOn/HealthCheck をすべて実装した。Phase 5 では開発体験を改善する4機能を実装する。

**対象要件**: FR-9.1〜FR-9.4

---

## アーキテクチャ

```
                      ┌───────────────────────────────────────────────────┐
                      │   Lecs Host Process                              │
                      │                                                   │
                      │   lecs run -f task-def.json                      │
                      │     │                                             │
                      │     ▼                                             │
                      │   TaskDefinition::from_file()                     │
                      │     │  volumes: [{name, host.sourcePath}]         │
                      │     │  containerDefinitions[].mountPoints          │
                      │     ▼                                             │
                      │   build_container_config()                        │
                      │     │  resolve_binds(mountPoints, volumes)        │
                      │     │  → binds: ["/host:/container:ro", ...]     │
                      │     ▼                                             │
                      │   build_bollard_config()                          │
                      │     │  HostConfig.binds = Some(binds)             │
                      │     ▼                                             │
                      │   Container Runtime API (bollard)                 │
                      │                                                   │
                      │   ┌─────────────────────────────────┐            │
                      │   │  stream_logs_until_signal()     │            │
                      │   │  \x1b[32m[app]\x1b[0m log...    │ ← 色分け  │
                      │   │  \x1b[33m[db]\x1b[0m  log...    │            │
                      │   └─────────────────────────────────┘            │
                      │                                                   │
                      │   lecs ps         lecs logs <name>              │
                      │     │                │                            │
                      │     ▼                ▼                            │
                      │   list_containers  find_container + stream_logs   │
                      │     │                                             │
                      │     ▼                                             │
                      │   format_table → stdout                           │
                      └───────────────────────────────────────────────────┘
                                        ▲
               ┌────────────────────────┼─────────────────────────┐
               │   lecs-<family> network                          │
               │                        │                          │
               │   ┌─────┐  ┌─────┐  ┌─────┐                     │
               │   │ app │  │ db  │  │redis│                     │
               │   │     │  │     │  │     │                     │
               │   │ /data ← /local/data (bind mount)            │
               │   └─────┘  └─────┘  └─────┘                     │
               └───────────────────────────────────────────────────┘
```

---

## モジュール配置

| ファイル | 責務 |
|---------|------|
| `src/taskdef/mod.rs` | `Volume`, `VolumeHost`, `MountPoint` 型定義 + `validate_mount_points()` |
| `src/container/mod.rs` | `ContainerConfig.binds` 追加、`ContainerInfo.image` 追加、`build_bollard_config()` 更新 |
| `src/cli/run.rs` | `resolve_binds()`, `container_color()`, `format_log_line()` |
| `src/cli/ps.rs` | **新規** — `lecs ps` コマンド実装 |
| `src/cli/logs.rs` | **新規** — `lecs logs` コマンド実装 |
| `src/cli/mod.rs` | `PsArgs`, `LogsArgs` 型定義、`Command` enum 拡張 |
| `src/main.rs` | `Ps`, `Logs` ディスパッチ追加 |

---

## 型定義

### taskdef 層（ECS 互換）

```rust
/// Task-level volume definition (ECS-compatible).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    /// Volume name (referenced by mountPoints).
    pub name: String,
    /// Host path for bind mount. None for Docker-managed volumes (skipped).
    #[serde(default)]
    pub host: Option<VolumeHost>,
}

/// Host path for bind mount volumes.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeHost {
    /// Absolute path on the host machine.
    pub source_path: String,
}

/// Container mount point referencing a task-level volume.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MountPoint {
    /// Name of the volume to mount (must match a Volume.name).
    pub source_volume: String,
    /// Absolute path inside the container.
    pub container_path: String,
    /// Mount as read-only (default: false).
    #[serde(default)]
    pub read_only: bool,
}
```

`TaskDefinition` に追加:
```rust
/// Task-level volume definitions.
#[serde(default)]
pub volumes: Vec<Volume>,
```

`ContainerDefinition` に追加:
```rust
/// Mount points referencing task-level volumes.
#[serde(default)]
pub mount_points: Vec<MountPoint>,
```

### container 層

```rust
// ContainerConfig に追加
/// Bind mount volumes (format: "host_path:container_path" or "host_path:container_path:ro").
pub binds: Vec<String>,

// ContainerInfo に追加
/// Container image name.
pub image: String,
```

### cli 層

```rust
#[derive(Parser)]
pub struct PsArgs {
    /// Filter by task family name.
    pub task: Option<String>,
}

#[derive(Parser)]
pub struct LogsArgs {
    /// Container name (e.g., "app" or "my-task-app").
    pub container: String,
    /// Follow log output (like tail -f).
    #[arg(short, long)]
    pub follow: bool,
}
```

---

## 公開 API

### ボリューム解決

```rust
/// Resolve mount points against task-level volumes into Docker bind mount strings.
///
/// Returns bind strings in format "host_path:container_path" or "host_path:container_path:ro".
/// Volumes without `host.source_path` (Docker-managed volumes) are skipped with a warning.
fn resolve_binds(mount_points: &[MountPoint], volumes: &[Volume]) -> Vec<String>
```

### ログ色分け

```rust
/// ANSI color codes for log multiplexing (12 distinct colors).
const COLORS: &[&str] = &[
    "32", "33", "34", "35", "36", "91",
    "92", "93", "94", "95", "96", "31",
];

/// Return the ANSI color code for a container at the given index.
fn container_color(index: usize) -> &'static str

/// Format a log line with ANSI color-coded container prefix.
fn format_log_line(name: &str, line: &str, color: &str) -> String
```

### ps コマンド

```rust
/// Execute the `ps` subcommand.
#[cfg(not(tarpaulin_include))]
pub async fn execute(args: &PsArgs, host: Option<&str>) -> Result<()>

/// List Lecs-managed containers (testable with mock).
#[allow(clippy::print_stdout)]
pub async fn execute_with_client(
    args: &PsArgs,
    client: &(impl ContainerRuntime + ?Sized),
) -> Result<()>

/// Format container list as a table string with aligned columns.
fn format_table(containers: &[ContainerInfo]) -> String
```

### logs コマンド

```rust
/// Execute the `logs` subcommand.
#[cfg(not(tarpaulin_include))]
pub async fn execute(args: &LogsArgs, host: Option<&str>) -> Result<()>

/// Find a container by name (exact match → contains fallback).
fn find_container<'a>(containers: &'a [ContainerInfo], query: &str) -> Option<&'a ContainerInfo>
```

---

## データフロー

### ボリュームマウント解決フロー

```
TaskDefinition::from_file()
    │
    ├── volumes: [
    │     { name: "data", host: { sourcePath: "/local/data" } },
    │     { name: "cache", host: null }  ← Docker-managed, スキップ対象
    │   ]
    │
    ├── containerDefinitions[0].mount_points: [
    │     { sourceVolume: "data", containerPath: "/app/data", readOnly: false },
    │     { sourceVolume: "cache", containerPath: "/tmp/cache", readOnly: true }
    │   ]
    │
    ▼ validate_mount_points()
    │  ✓ "data" は volumes に存在
    │  ✓ "cache" は volumes に存在
    │
    ▼ build_container_config(def, volumes, ...)
    │
    ▼ resolve_binds(mount_points, volumes)
    │  "data" → host.sourcePath = "/local/data" → "/local/data:/app/data"
    │  "cache" → host = None → tracing::warn! → スキップ
    │
    ▼ ContainerConfig { binds: ["/local/data:/app/data"] }
    │
    ▼ build_bollard_config()
    │  HostConfig { binds: Some(["/local/data:/app/data"]) }
    │
    ▼ Docker/Podman API → コンテナ作成
```

### `lecs ps` データフロー

```
lecs ps [family]
    │
    ▼ ContainerClient::connect()
    │
    ▼ list_containers(task_filter)
    │  Docker API: GET /containers/json?filters={"label":["lecs.managed=true"]}
    │  → Vec<ContainerInfo> { id, name, image, family, state }
    │
    ▼ format_table(&containers)
    │  カラム幅計算: max(header.len(), max(value.len())) + padding
    │  → String (table format)
    │
    ▼ println!("{table}")
```

### `lecs logs` データフロー

```
lecs logs <container> [--follow]
    │
    ▼ ContainerClient::connect()
    │
    ▼ list_containers(None)
    │  → Vec<ContainerInfo>
    │
    ▼ find_container(&containers, query)
    │  1. name == query (完全一致)
    │  2. name.contains(query) (部分一致フォールバック)
    │  → Some(&ContainerInfo) or None → anyhow::bail!
    │
    ├── follow=false:
    │   ▼ docker.logs(id, LogsOptions { follow: false, stdout: true, stderr: true })
    │   ▼ 全ログ出力 → 終了
    │
    └── follow=true:
        ▼ stream_logs(id)  // follow: true
        ▼ Ctrl+C まで出力継続
```

---

## エラーハンドリング

| ケース | 挙動 | エラー型 | 理由 |
|--------|------|---------|------|
| `mount_points` の `source_volume` が `volumes` にない | **hard error** | `TaskDefError::Validation` | タスク定義の整合性エラー |
| `host.source_path` が空文字 | **hard error** | `TaskDefError::Validation` | bind mount に必須 |
| `container_path` が空文字 | **hard error** | `TaskDefError::Validation` | コンテナ内パスに必須 |
| ボリュームの `host` が `None` | **warning** + skip | `tracing::warn` | EFS/Docker-managed volume は非対応 |
| `lecs ps` でコンテナなし | 正常終了 | - | "No lecs containers found." を表示 |
| `lecs logs` でコンテナが見つからない | **hard error** | `anyhow::bail!` | ユーザの指定ミス |
| `lecs logs` でランタイム未起動 | **hard error** | `ContainerError::RuntimeNotRunning` | 既存のエラーハンドリング |
| 色コード割り当て（コンテナ数 > 12） | 正常動作 | - | 色を循環使用 |

---

## ECS ボリューム互換性

### サポート範囲

| ECS ボリュームタイプ | サポート | 備考 |
|---------------------|---------|------|
| Bind mount (`host.sourcePath` 指定あり) | ✅ | Docker `HostConfig.binds` にマッピング |
| Docker-managed volume (`host` なし) | ⚠️ skip | 警告ログを出力してスキップ |
| EFS volume | ❌ skip | ローカルでは再現不可、警告ログ |
| FSx for Windows File Server | ❌ skip | ローカルでは再現不可、警告ログ |

### ECS タスク定義フォーマット（入力例）

```json
{
  "family": "my-app",
  "volumes": [
    {
      "name": "app-data",
      "host": { "sourcePath": "/home/user/data" }
    },
    {
      "name": "tmp-cache"
    }
  ],
  "containerDefinitions": [
    {
      "name": "app",
      "image": "my-app:latest",
      "mountPoints": [
        {
          "sourceVolume": "app-data",
          "containerPath": "/data",
          "readOnly": false
        },
        {
          "sourceVolume": "tmp-cache",
          "containerPath": "/tmp/cache",
          "readOnly": true
        }
      ]
    }
  ]
}
```

### Docker bind mount 出力

```
/home/user/data:/data          ← app-data: host.sourcePath あり
(skip with warning)            ← tmp-cache: host なし
```

---

## 技術選定

### ログ色分け: ANSI エスケープコード直接記述

| 候補 | 判断 | 理由 |
|------|------|------|
| 直接 ANSI コード | **採用** | 依存ゼロ。プレフィックスのみの着色で十分シンプル |
| `colored` クレート | 不採用 | 機能過剰。Windows Terminal 対応は本ツールのスコープ外 |
| `owo-colors` クレート | 不採用 | 軽量だが依存追加の価値がない |

### テーブルフォーマット: 手動実装

| 候補 | 判断 | 理由 |
|------|------|------|
| 手動カラム幅計算 | **採用** | 4カラムのみ。Phase 7 で `--output json/wide` 拡張時にも柔軟 |
| `comfy-table` クレート | 不採用 | 依存追加の価値がない |
| `tabled` クレート | 不採用 | 機能過剰 |

### コンテナ検索: 完全一致 + 部分一致フォールバック

ECS と異なりローカルではコンテナ名に `{family}-` プレフィックスが付くため、ユーザが `app` と入力して `my-task-app` にマッチできるように部分一致をフォールバックとして提供。

---

## スコープ外

- Docker-managed volume（`host` なしボリューム）の Docker volume 作成
- EFS / FSx ボリュームのエミュレーション
- `--output json` / `--output wide`（Phase 7 で対応）
- `lecs ps` のリソース使用量表示（Phase 7 で対応）
- ターミナル幅検出によるカラム幅自動調整
- ログのタイムスタンプ付与
- Windows コンソールの ANSI エスケープ互換性ハンドリング

---

## テスト戦略

| テスト対象 | テスト数（目標） | 方法 |
|-----------|---------|------|
| taskdef パース（volumes, mountPoints） | 6 | JSON パース + フィールド検証 |
| taskdef バリデーション（mount_points） | 3 | エラーケース検証（未知ボリューム、空パス） |
| container: bollard config binds | 2 | `build_bollard_config` の出力検証 |
| cli/run: `resolve_binds` | 3 | 純粋関数テスト |
| cli/run: `container_color` | 2 | インデックス → カラーコード + 循環 |
| cli/run: `format_log_line` | 1 | ANSI 出力フォーマット検証 |
| cli/mod: CLI パース（ps, logs） | 4 | clap パーステスト |
| cli/ps: `format_table` | 2 | テーブル出力フォーマット |
| cli/ps: `execute_with_client` | 3 | モックテスト（0件、1件、フィルター） |
| cli/logs: `find_container` | 3 | 完全一致、部分一致、未検出 |
| 既存テスト修正（mount_points, binds 追加） | - | 全箇所に `vec![]` 追加 |
| **合計** | **29+** | |
