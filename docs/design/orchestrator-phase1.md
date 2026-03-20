# コンテナライフサイクル設計書（Phase 1）

## 概要

`egret run` および `egret stop` コマンドのコンテナライフサイクル管理を定義する。
Phase 1 では dependsOn DAG やヘルスチェックは対象外とし、全コンテナの並行起動・停止に限定する。

## `egret run` フロー

```
egret run -f task-def.json
         │
         ▼
┌─────────────────────┐
│ 1. JSON パース       │  TaskDefinition::from_file()
│    + バリデーション   │
└─────────┬───────────┘
          │
          ▼
┌─────────────────────┐
│ 2. Docker 接続確認   │  DockerClient::connect()
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
│ 4. 各コンテナを作成・起動        │  並行処理
│    for each containerDefinition │
│    ├─ create_container()        │
│    ├─ start_container()         │
│    └─ spawn log stream task     │
└─────────┬───────────────────────┘
          │
          ▼
┌─────────────────────┐
│ 5. シグナル待機      │  Ctrl+C (SIGINT) を待つ
│    + ログストリーム   │  全コンテナのログを表示
└─────────┬───────────┘
          │ SIGINT
          ▼
┌─────────────────────┐
│ 6. クリーンアップ    │  stop → remove → network remove
└─────────────────────┘
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
│ 3. コンテナ停止・削除     │  並行処理
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

## CLI 変更

### `egret run`

現在の同期 `execute` を async に変更する:

```rust
// src/cli/run.rs
use crate::docker::DockerClient;
use crate::taskdef::TaskDefinition;

pub async fn execute(args: &RunArgs) -> anyhow::Result<()> {
    // 1. パース
    let task_def = TaskDefinition::from_file(&args.task_definition)?;
    tracing::info!(family = %task_def.family, "Parsed task definition");

    // 2. Docker 接続
    let client = DockerClient::connect().await?;

    // 3. ネットワーク作成
    let network_name = client.create_network(&task_def.family).await?;
    tracing::info!(network = %network_name, "Created network");

    // 4. コンテナ作成・起動
    let mut container_ids = Vec::new();
    for container_def in &task_def.container_definitions {
        let config = build_container_config(&task_def.family, container_def, &network_name);
        let id = client.create_container(&config).await?;
        client.start_container(&id).await?;
        container_ids.push(id.clone());
        tracing::info!(
            container = %container_def.name,
            "Started container"
        );
    }

    // 5. ログストリーム + シグナル待機
    stream_logs_until_signal(&client, &container_ids).await;

    // 6. クリーンアップ
    cleanup(&client, &container_ids, &network_name).await?;

    Ok(())
}
```

### `egret stop`

```rust
// src/cli/stop.rs
use crate::docker::DockerClient;

pub async fn execute(args: &StopArgs) -> anyhow::Result<()> {
    let client = DockerClient::connect().await?;

    let task_filter = if args.all {
        None
    } else if let Some(task) = &args.task {
        Some(task.as_str())
    } else {
        anyhow::bail!("Specify a task name or use --all to stop all tasks.");
    };

    // コンテナ停止・削除
    let containers = client.list_containers(task_filter).await?;
    for container in &containers {
        client.stop_container(&container.id).await?;
        client.remove_container(&container.id).await?;
        tracing::info!(container = %container.name, "Stopped and removed");
    }

    // ネットワーク削除
    let networks = client.list_networks(task_filter).await?;
    for network in &networks {
        client.remove_network(&network.name).await?;
        tracing::info!(network = %network.name, "Removed network");
    }

    Ok(())
}
```

## `main.rs` の変更

```rust
// 変更前
fn main() {
    // ...
    match cli.command {
        cli::Command::Run(args) => cli::run::execute(&args),
        // ...
    }
}

// 変更後
#[tokio::main]
async fn main() -> anyhow::Result<()> {
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

## TaskDefinition → ContainerConfig 変換

```rust
/// TaskDefinition のコンテナ定義から DockerClient 用の設定を生成
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

    let env = def.environment
        .iter()
        .map(|e| format!("{}={}", e.name, e.value))
        .collect();

    let port_mappings = def.port_mappings
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
        labels,
    }
}
```

## ログストリーム設計

各コンテナのログを色分けして表示する:

```
[app]   2024-01-01T00:00:00Z Starting nginx...
[redis] 2024-01-01T00:00:00Z Ready to accept connections
[app]   2024-01-01T00:00:01Z Listening on port 80
```

- 各コンテナのログストリームを `tokio::spawn` で並行実行
- コンテナ名をプレフィックスとして付与
- `tokio::signal::ctrl_c()` でシグナルを待機し、受信後にクリーンアップ

```rust
async fn stream_logs_until_signal(
    client: &DockerClient,
    container_ids: &[String],
) {
    let mut handles = Vec::new();

    for id in container_ids {
        let stream = client.stream_logs(id).await;
        // tokio::spawn で各コンテナのログを非同期に処理
        // ...
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

## Graceful Shutdown

1. `Ctrl+C` (SIGINT) を受信
2. 全ログストリームタスクを abort
3. 全コンテナを停止（タイムアウト 10 秒）
4. 全コンテナを削除
5. ネットワークを削除
6. プロセス終了

停止中にエラーが発生した場合は、ログに警告を出して次のコンテナの処理に進む（ベストエフォート）。

## テスト戦略

| テスト対象 | テスト方法 |
|---|---|
| `build_container_config` | ユニットテスト: TaskDef → Config 変換の正確性 |
| ラベル生成 | ユニットテスト: 正しいラベルが設定されるか |
| コンテナ名生成 | ユニットテスト: `<family>-<name>` 形式 |
| 環境変数変換 | ユニットテスト: `KEY=VALUE` 形式への変換 |
| ポートマッピング変換 | ユニットテスト: host_port デフォルト値の処理 |
| 全体フロー | 手動テスト: Docker 環境で `cargo run` |

## Phase 1 での制限事項

- コンテナの起動順序制御なし（全コンテナを並行起動）→ Phase 4 で dependsOn 対応
- ヘルスチェック未対応 → Phase 4
- essential コンテナ停止時の連動停止未対応 → Phase 4
- ボリュームマウント未対応 → Phase 5
- ログの色分けは Phase 5 で実装（Phase 1 ではプレフィックスのみ）
