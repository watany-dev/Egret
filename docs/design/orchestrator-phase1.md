# コンテナライフサイクル設計書（Phase 1）

## 概要

`egret run` および `egret stop` コマンドのコンテナライフサイクル管理を定義する。
Phase 1 では dependsOn DAG やヘルスチェックは対象外とし、全コンテナの並行起動・停止に限定する。

## モジュール配置方針

Phase 1 のライフサイクルロジックは以下のように配置する:

| ファイル | 責務 |
|---------|------|
| `src/cli/run.rs` | `egret run` のエントリポイント。パース → ランタイム接続 → 起動 → ログ → クリーンアップの全体フロー |
| `src/cli/stop.rs` | `egret stop` のエントリポイント。ラベル検索 → 停止 → 削除のフロー |
| `src/container/mod.rs` | コンテナランタイム API 操作のみ（設計書: `container.md`）|
| `src/taskdef/mod.rs` | JSON パースのみ（設計書: `taskdef.md`）|

`build_container_config` 関数は `src/cli/run.rs` 内のプライベート関数として配置する。
CLI 層が「TaskDefinition → ContainerConfig」の変換責務を持ち、container モジュールはランタイム API 操作に集中する。

`src/orchestrator/mod.rs` は Phase 4 で dependsOn DAG 解決、ヘルスチェック監視、essential コンテナ監視の責務を担う。`run_task()` は `orchestrate_startup()` に委譲し、DAG ベースでコンテナを起動する。詳細は `phase4-dependson-healthcheck.md` を参照。

## `main.rs` の変更

```rust
use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

mod cli;
mod container;
mod credentials;
mod metadata;
mod orchestrator;
mod overrides;
mod secrets;
mod taskdef;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = cli::Cli::parse();

    match cli.command {
        cli::Command::Run(args) => cli::run::execute(&args, cli.host.as_deref()).await?,
        cli::Command::Stop(args) => cli::stop::execute(&args, cli.host.as_deref()).await?,
        cli::Command::Version => cli::version::execute(),
    }

    Ok(())
}
```

変更点:
- `fn main()` → `#[tokio::main] async fn main() -> Result<()>`
- 全モジュール宣言追加（`container`, `credentials`, `metadata`, `orchestrator`, `overrides`, `secrets`, `taskdef`）
- `cli::run::execute` と `cli::stop::execute` に `host` パラメータを渡す
- `cli.host.as_deref()` で `Option<String>` → `Option<&str>` 変換

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
│ 2. コンテナランタイム接続確認   │  ContainerClient::connect()
│                     │  エラー → "Container runtime is not running" 表示
└─────────┬───────────┘
          │
          ▼
┌─────────────────────┐
│ 3. ネットワーク作成  │  ContainerClient::create_network()
│    egret-<family>    │
└─────────┬───────────┘
          │
          ▼
┌─────────────────────────────────┐
│ 4. DAG ベースでコンテナ起動      │
│    orchestrate_startup() で     │
│    dependsOn の順序に従い       │
│    レイヤーごとに起動・条件待機  │
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
use crate::container::{ContainerConfig, ContainerClient, PortMappingConfig};
use crate::taskdef::{ContainerDefinition, TaskDefinition};

pub async fn execute(args: &RunArgs, host: Option<&str>) -> Result<()> {
    // 1. パース
    let mut task_def = TaskDefinition::from_file(&args.task_definition)?;
    tracing::info!(family = %task_def.family, "Parsed task definition");

    // 1.5. Override 適用（Phase 2 で追加）
    if let Some(override_path) = &args.r#override {
        let overrides = OverrideConfig::from_file(override_path)?;
        overrides.apply(&mut task_def);
    }

    // 1.6. Secrets 解決（Phase 2 で追加）
    if let Some(secrets_path) = &args.secrets {
        let resolver = SecretsResolver::from_file(secrets_path)?;
        // secrets を環境変数に変換
    }

    // 2. コンテナランタイム接続
    let client = Arc::new(ContainerClient::connect(host).await?);

    // 2.5. メタデータサーバー起動（Phase 3 で追加）
    let metadata_port = if args.no_metadata {
        None
    } else {
        // AWS クレデンシャルロード + MetadataServer::start()
        // ...
        Some(port)
    };

    // 3. ネットワーク作成 + DAG ベース起動（Phase 4 で orchestrate_startup に委譲）
    let (network_name, containers) = run_task(&*client, &task_def, metadata_port).await?;

    // 4. ログストリーム + シグナル待機
    stream_logs_until_signal(&client, &containers).await;

    // 5. クリーンアップ（メタデータサーバー + コンテナ）
    cleanup(&*client, &containers, &network_name).await;

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
    metadata_port: Option<u16>,  // Phase 3 で追加
) -> ContainerConfig {
    let labels = HashMap::from([
        ("egret.managed".into(), "true".into()),
        ("egret.task".into(), family.into()),
        ("egret.container".into(), def.name.clone()),
    ]);

    let mut env: Vec<String> = def
        .environment
        .iter()
        .map(|e| format!("{}={}", e.name, e.value))
        .collect();

    // Phase 3: メタデータ/クレデンシャル環境変数注入
    if let Some(port) = metadata_port {
        env.push(format!(
            "ECS_CONTAINER_METADATA_URI_V4=http://host.docker.internal:{port}/v4/{}", def.name
        ));
        env.push(format!(
            "AWS_CONTAINER_CREDENTIALS_FULL_URI=http://host.docker.internal:{port}/credentials"
        ));
    }

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
        extra_hosts: vec!["host.docker.internal:host-gateway".to_string()],  // Phase 3
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

`tokio::spawn` は `'static` ライフタイムを要求するため、`ContainerClient` を `Arc` で共有する:

```rust
use std::sync::Arc;
use tokio::task::JoinHandle;

async fn stream_logs_until_signal(
    client: &Arc<ContainerClient>,
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
│ 1. コンテナランタイム接続確認        │
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
use crate::container::ContainerClient;

pub async fn execute(args: &StopArgs, host: Option<&str>) -> Result<()> {
    let client = ContainerClient::connect(host).await?;

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
    client: &ContainerClient,
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
| メタデータ環境変数注入 | ユニットテスト: metadata_port ありで ECS 環境変数が注入される | `src/cli/run.rs` |
| extra_hosts 設定 | ユニットテスト: `host.docker.internal:host-gateway` が含まれる | `src/cli/run.rs` |
| 全体フロー | 手動テスト: Docker/Podman 環境で `cargo run` | — |

## Phase 4 で解消された制限事項

以下は Phase 1 時点での制限事項であったが、Phase 4 で実装済み:
- ~~コンテナの起動順序制御なし~~ → `orchestrate_startup()` で dependsOn DAG 解決
- ~~ヘルスチェック未対応~~ → `HealthCheck` / `HealthCheckConfig` で Docker HEALTHCHECK 設定
- ~~essential コンテナ停止時の連動停止未対応~~ → `watch_essential_exit()` で監視可能

## 残存する制限事項

- ボリュームマウント未対応 → Phase 5
- ログの色分けは Phase 5 で実装（現在はプレフィックスのみ）
