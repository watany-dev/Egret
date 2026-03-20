# コンテナライフサイクル設計書（Phase 1）

## 概要

`egret run` および `egret stop` コマンドのコンテナライフサイクル管理を定義する。
Phase 1 では dependsOn DAG やヘルスチェックは対象外とし、全コンテナの並行起動・停止に限定する。

## モジュール配置方針

Phase 1 のライフサイクルロジックは以下のように配置する:

| ファイル | 責務 |
|---------|------|
| `src/cli/run.rs` | `egret run` のエントリポイント。パース → Docker 接続 → 起動 → ログ → クリーンアップの全体フロー |
| `src/cli/stop.rs` | `egret stop` のエントリポイント。ラベル検索 → 停止 → 削除のフロー |
| `src/docker/mod.rs` | Docker API 操作のみ（設計書: `docker.md`）|
| `src/taskdef/mod.rs` | JSON パースのみ（設計書: `taskdef.md`）|

`build_container_config` 関数は `src/cli/run.rs` 内のプライベート関数として配置する。
CLI 層が「TaskDefinition → ContainerConfig」の変換責務を持ち、docker モジュールは Docker API 操作に集中する。

`src/orchestrator/mod.rs` は Phase 1 では**使用しない**。Phase 4 で dependsOn DAG 解決とヘルスチェック監視の責務を担う予定。

## `main.rs` の変更

```rust
use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

mod cli;
mod docker;
mod taskdef;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = cli::Cli::parse();

    match cli.command {
        cli::Command::Run(args) => cli::run::execute(&args).await?,
        cli::Command::Stop(args) => cli::stop::execute(&args).await?,
        cli::Command::Version => cli::version::execute(),
    }

    Ok(())
}
```

変更点:
- `fn main()` → `#[tokio::main] async fn main() -> Result<()>`
- `mod taskdef;` と `mod docker;` を追加
- `cli::run::execute` と `cli::stop::execute` を `.await?` で呼び出し

## `egret run` フロー

```
egret run -f task-def.json
         │
         ▼
┌─────────────────────┐
│ 1. JSON パース       │  TaskDefinition::from_file()
│    + バリデーション   │  エラー → anyhow に変換して表示
└─────────┬───────────┘
          │
          ▼
┌─────────────────────┐
│ 2. Docker 接続確認   │  DockerClient::connect()
│                     │  エラー → "Docker daemon is not running" 表示
└─────────┬───────────┘
          │
          ▼
┌─────────────────────┐
│ 3. ネットワーク作成  │  DockerClient::create_network()
│    egret-<family>    │
└─────────┬───────────┘
          │
          ▼
┌─────────────────────────────────┐
│ 4. 各コンテナを作成・起動        │
│    for each containerDefinition │
│    ├─ build_container_config()  │
│    ├─ create_container()        │
│    └─ start_container()         │
└─────────┬───────────────────────┘
          │
          ▼
┌─────────────────────┐
│ 5. ログストリーム    │  tokio::spawn × N
│    + シグナル待機    │  Ctrl+C (SIGINT) を待つ
└─────────┬───────────┘
          │ SIGINT
          ▼
┌─────────────────────┐
│ 6. クリーンアップ    │  stop → remove → network remove
│    (ベストエフォート) │
└─────────────────────┘
```

### `egret run` 実装

```rust
// src/cli/run.rs
use std::sync::Arc;

use anyhow::Result;
use futures_util::StreamExt;

use super::RunArgs;
use crate::docker::{ContainerConfig, DockerClient, PortMappingConfig};
use crate::taskdef::{ContainerDefinition, TaskDefinition};

pub async fn execute(args: &RunArgs) -> Result<()> {
    // 1. パース
    let task_def = TaskDefinition::from_file(&args.task_definition)?;
    tracing::info!(family = %task_def.family, "Parsed task definition");

    // 2. Docker 接続
    let client = Arc::new(DockerClient::connect().await?);

    // 3. ネットワーク作成
    let network_name = client.create_network(&task_def.family).await?;
    tracing::info!(network = %network_name, "Created network");

    // 4. コンテナ作成・起動
    let mut container_ids = Vec::new();
    for container_def in &task_def.container_definitions {
        let config = build_container_config(
            &task_def.family,
            container_def,
            &network_name,
        );
        let id = client.create_container(&config).await?;
        client.start_container(&id).await?;
        container_ids.push((id, container_def.name.clone()));
        tracing::info!(container = %container_def.name, "Started container");
    }

    // 5. ログストリーム + シグナル待機
    stream_logs_until_signal(&client, &container_ids).await;

    // 6. クリーンアップ
    cleanup(&client, &container_ids, &network_name).await;

    Ok(())
}
```

## TaskDefinition → ContainerConfig 変換

`src/cli/run.rs` 内のプライベート関数:

```rust
use std::collections::HashMap;

fn build_container_config(
    family: &str,
    def: &ContainerDefinition,
    network: &str,
) -> ContainerConfig {
    let labels = HashMap::from([
        ("egret.managed".into(), "true".into()),
        ("egret.task".into(), family.into()),
        ("egret.container".into(), def.name.clone()),
    ]);

    let env = def
        .environment
        .iter()
        .map(|e| format!("{}={}", e.name, e.value))
        .collect();

    let port_mappings = def
        .port_mappings
        .iter()
        .map(|p| PortMappingConfig {
            host_port: p.host_port.unwrap_or(p.container_port),
            container_port: p.container_port,
            protocol: p.protocol.clone(),
        })
        .collect();

    ContainerConfig {
        name: format!("{family}-{}", def.name),
        image: def.image.clone(),
        command: def.command.clone(),
        entry_point: def.entry_point.clone(),
        env,
        port_mappings,
        network: network.into(),
        network_aliases: vec![def.name.clone()],
        labels,
    }
}
```

## ログストリーム設計

各コンテナのログをプレフィックス付きで表示する:

```
[app]   2024-01-01T00:00:00Z Starting nginx...
[redis] 2024-01-01T00:00:00Z Ready to accept connections
[app]   2024-01-01T00:00:01Z Listening on port 80
```

### 所有権とライフタイム

`tokio::spawn` は `'static` ライフタイムを要求するため、`DockerClient` を `Arc` で共有する:

```rust
use std::sync::Arc;
use tokio::task::JoinHandle;

async fn stream_logs_until_signal(
    client: &Arc<DockerClient>,
    containers: &[(String, String)], // (id, name)
) {
    let mut handles: Vec<JoinHandle<()>> = Vec::new();

    for (id, name) in containers {
        let client = Arc::clone(client);
        let id = id.clone();
        let name = name.clone();

        handles.push(tokio::spawn(async move {
            let mut stream = client.stream_logs(&id);
            while let Some(result) = stream.next().await {
                match result {
                    Ok(line) => println!("[{name}] {line}"),
                    Err(e) => {
                        tracing::warn!(
                            container = %name,
                            error = %e,
                            "Log stream error"
                        );
                        break;
                    }
                }
            }
        }));
    }

    // Ctrl+C 待機
    tokio::signal::ctrl_c().await.ok();
    tracing::info!("Received SIGINT, shutting down...");

    // ログタスクを abort
    for handle in &handles {
        handle.abort();
    }
}
```

## `egret stop` フロー

```
egret stop [<task>] [--all]
         │
         ▼
┌──────────────────────────┐
│ 1. Docker 接続確認        │
└─────────┬────────────────┘
          │
          ▼
┌──────────────────────────┐
│ 2. Egret コンテナ検索     │  ラベル filter:
│                          │  egret.managed=true
│                          │  egret.task=<task> (指定時)
└─────────┬────────────────┘
          │
          ▼
┌──────────────────────────┐
│ 3. コンテナ停止・削除     │  ベストエフォート
│    for each container    │
│    ├─ stop_container()   │
│    └─ remove_container() │
└─────────┬────────────────┘
          │
          ▼
┌──────────────────────────┐
│ 4. ネットワーク削除       │
│    for each network      │
│    └─ remove_network()   │
└──────────────────────────┘
```

### `egret stop` 実装

```rust
// src/cli/stop.rs
use anyhow::Result;

use super::StopArgs;
use crate::docker::DockerClient;

pub async fn execute(args: &StopArgs) -> Result<()> {
    let client = DockerClient::connect().await?;

    let task_filter = if args.all {
        None
    } else if let Some(task) = &args.task {
        Some(task.as_str())
    } else {
        anyhow::bail!("Specify a task name or use --all to stop all tasks.");
    };

    // コンテナ停止・削除（ベストエフォート）
    let containers = client.list_containers(task_filter).await?;
    for container in &containers {
        if let Err(e) = client.stop_container(&container.id).await {
            tracing::warn!(
                container = %container.name,
                error = %e,
                "Failed to stop container"
            );
        }
        if let Err(e) = client.remove_container(&container.id).await {
            tracing::warn!(
                container = %container.name,
                error = %e,
                "Failed to remove container"
            );
        }
        tracing::info!(container = %container.name, "Stopped and removed");
    }

    // ネットワーク削除
    let networks = client.list_networks(task_filter).await?;
    for network in &networks {
        if let Err(e) = client.remove_network(&network.name).await {
            tracing::warn!(
                network = %network.name,
                error = %e,
                "Failed to remove network"
            );
        }
        tracing::info!(network = %network.name, "Removed network");
    }

    Ok(())
}
```

## Graceful Shutdown

1. `Ctrl+C` (SIGINT) を受信
2. 全ログストリームタスクを abort
3. 全コンテナを停止（タイムアウト 10 秒）
4. 全コンテナを削除
5. ネットワークを削除
6. プロセス終了

### クリーンアップのエラーハンドリング

停止中にエラーが発生した場合は、ログに警告を出して次のリソースの処理に進む（ベストエフォート）。
クリーンアップ全体がエラーで中断しないようにする。

```rust
async fn cleanup(
    client: &DockerClient,
    containers: &[(String, String)], // (id, name)
    network: &str,
) {
    for (id, name) in containers {
        if let Err(e) = client.stop_container(id).await {
            tracing::warn!(container = %name, error = %e, "Failed to stop container");
        }
        if let Err(e) = client.remove_container(id).await {
            tracing::warn!(container = %name, error = %e, "Failed to remove container");
        }
        tracing::info!(container = %name, "Cleaned up");
    }

    if let Err(e) = client.remove_network(network).await {
        tracing::warn!(network = %network, error = %e, "Failed to remove network");
    }
    tracing::info!(network = %network, "Network removed");
}
```

`cleanup` は戻り値を `()` にする。エラーは全て警告ログに出力し、呼び出し元にはエラーを伝搬しない。

## テスト戦略

| テスト対象 | テスト方法 | ファイル |
|---|---|---|
| `build_container_config` | ユニットテスト: TaskDef → Config 変換の正確性 | `src/cli/run.rs` |
| ラベル生成 | ユニットテスト: 正しいラベルが設定されるか | `src/cli/run.rs` |
| コンテナ名生成 | ユニットテスト: `<family>-<name>` 形式 | `src/cli/run.rs` |
| 環境変数変換 | ユニットテスト: `KEY=VALUE` 形式への変換 | `src/cli/run.rs` |
| ポートマッピング変換 | ユニットテスト: host_port デフォルト値の処理 | `src/cli/run.rs` |
| 全体フロー | 手動テスト: Docker 環境で `cargo run` | — |

## Phase 1 での制限事項

- コンテナの起動順序制御なし（全コンテナを順次起動）→ Phase 4 で dependsOn 対応
- ヘルスチェック未対応 → Phase 4
- essential コンテナ停止時の連動停止未対応 → Phase 4
- ボリュームマウント未対応 → Phase 5
- ログの色分けは Phase 5 で実装（Phase 1 ではプレフィックスのみ）
