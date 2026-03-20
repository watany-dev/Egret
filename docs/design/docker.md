# Docker クライアント設計書

## 概要

bollard クレートを通じて Docker Engine API と連携するモジュール。
コンテナ・ネットワークの作成・起動・停止・削除およびログストリームを提供する。
`src/docker/mod.rs` に実装する。

## 設計方針

- bollard の API を薄くラップし、Egret 固有のロジック（ラベル管理、命名規則）をカプセル化
- すべての操作は `async` で提供
- Docker デーモンへの接続はデフォルトの Unix ソケット（`/var/run/docker.sock`）を使用

## 技術選定

| 候補 | 判断 | 理由 |
|------|------|------|
| `bollard` | **採用** | 純 Rust 実装の Docker Engine API クライアント。async/await 対応。活発にメンテナンスされている |
| `docker-api` | 不採用 | bollard より利用者が少なく、API カバレッジも限定的 |
| `shiplift` | 不採用 | メンテナンスが停滞（最終リリースが古い） |
| Docker CLI ラップ | 不採用 | プロセス生成のオーバーヘッド、出力パースの脆弱性、エラーハンドリングの困難さ |

`futures-util` は bollard が返す `Stream` を処理するために必要。

### 依存クレート

```toml
[dependencies]
bollard = "0.18"
futures-util = "0.3"
```

bollard `0.18` は本設計書作成時点（2026-03）の最新安定版。Docker Engine API v1.44+ に対応。

## ラベル戦略

Egret が管理するリソースを識別するために、Docker ラベルを付与する:

| ラベルキー | 値 | 用途 |
|---|---|---|
| `egret.managed` | `"true"` | Egret が作成したリソースの識別 |
| `egret.task` | `<family>` | タスクファミリー名 |
| `egret.container` | `<container-name>` | コンテナ定義名（コンテナのみ） |

ネットワークにも `egret.managed` と `egret.task` ラベルを付与する。

## 型定義

```rust
use std::collections::HashMap;

use bollard::Docker;

/// Egret 用 Docker クライアント
pub struct DockerClient {
    docker: Docker,
}

/// コンテナ作成時の設定
pub struct ContainerConfig {
    /// コンテナ名（`<family>-<container_name>` 形式）
    pub name: String,
    /// Docker イメージ
    pub image: String,
    /// CMD
    pub command: Vec<String>,
    /// ENTRYPOINT
    pub entry_point: Vec<String>,
    /// 環境変数（`KEY=VALUE` 形式）
    pub env: Vec<String>,
    /// ポートマッピング
    pub port_mappings: Vec<PortMappingConfig>,
    /// 接続するネットワーク名
    pub network: String,
    /// ネットワーク内のエイリアス（コンテナ定義の name）
    pub network_aliases: Vec<String>,
    /// Docker ラベル
    pub labels: HashMap<String, String>,
}

/// ポートマッピング設定
pub struct PortMappingConfig {
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: String,
}

/// Egret が管理するコンテナの情報
pub struct ContainerInfo {
    /// Docker コンテナ ID
    pub id: String,
    /// コンテナ名
    pub name: String,
    /// タスクファミリー名（egret.task ラベルの値）
    pub family: String,
    /// コンテナの状態（running, exited 等）
    pub state: String,
}

/// Egret が管理するネットワークの情報
pub struct NetworkInfo {
    /// Docker ネットワーク ID
    pub id: String,
    /// ネットワーク名
    pub name: String,
}
```

## エラー型

```rust
#[derive(Debug, thiserror::Error)]
pub enum DockerError {
    /// Docker デーモンに接続できない
    #[error("Docker daemon is not running. Please start Docker and try again.")]
    DaemonNotRunning,

    /// Docker API エラー（bollard からの伝搬）
    #[error("Docker API error: {0}")]
    Api(#[from] bollard::errors::Error),
}
```

CLI 層で `DockerError` を `anyhow` に変換して表示する。
`DaemonNotRunning` はユーザーフレンドリーなメッセージを提供するために分離する。

## 公開 API

```rust
use futures_util::Stream;

impl DockerClient {
    /// Docker デーモンに接続し、ping で接続確認を行う
    /// 接続失敗時は DockerError::DaemonNotRunning を返す
    pub async fn connect() -> Result<Self, DockerError>;

    // --- ネットワーク ---

    /// Egret 専用ネットワークを作成する
    /// ネットワーク名: `egret-<family>`
    /// ドライバ: bridge（コンテナ名での DNS 解決が有効）
    /// 同名ネットワークが既存の場合はそのまま再利用する
    pub async fn create_network(
        &self,
        family: &str,
    ) -> Result<String, DockerError>;

    /// ネットワークを削除する
    pub async fn remove_network(&self, name: &str) -> Result<(), DockerError>;

    /// Egret が管理するネットワーク一覧を取得する
    /// task_filter: Some(<family>) で特定タスクに絞り込み、None で全て
    pub async fn list_networks(
        &self,
        task_filter: Option<&str>,
    ) -> Result<Vec<NetworkInfo>, DockerError>;

    // --- コンテナ ---

    /// コンテナを作成する（起動はしない）
    /// 戻り値はコンテナ ID
    pub async fn create_container(
        &self,
        config: &ContainerConfig,
    ) -> Result<String, DockerError>;

    /// コンテナを起動する
    pub async fn start_container(&self, id: &str) -> Result<(), DockerError>;

    /// コンテナを停止する（タイムアウト: 10秒、超過時は kill）
    pub async fn stop_container(&self, id: &str) -> Result<(), DockerError>;

    /// コンテナを削除する
    pub async fn remove_container(&self, id: &str) -> Result<(), DockerError>;

    /// Egret が管理するコンテナ一覧を取得する
    /// task_filter: Some(<family>) で特定タスクに絞り込み、None で全て
    pub async fn list_containers(
        &self,
        task_filter: Option<&str>,
    ) -> Result<Vec<ContainerInfo>, DockerError>;

    // --- ログ ---

    /// コンテナのログをストリームとして返す
    /// follow=true でリアルタイムストリーム
    pub fn stream_logs(
        &self,
        id: &str,
    ) -> impl Stream<Item = Result<String, DockerError>> + '_;
}
```

## ネットワーク設計

```
┌─────────────────────────────────────────┐
│        egret-<family> (bridge)          │
│                                         │
│  ┌───────────┐     ┌───────────┐       │
│  │  app       │────▶│  redis    │       │
│  │ (container)│     │(container)│       │
│  └───────────┘     └───────────┘       │
│       DNS: app          DNS: redis      │
└─────────────────────────────────────────┘
         │
         ▼ port mapping
    host:8080 → app:80
```

- Docker bridge ネットワーク内では、コンテナ名がそのまま DNS 名として解決される
- ECS の `awsvpc` ネットワークモードに近い挙動を bridge + DNS で再現
- コンテナ名は `<family>-<container_name>` 形式（例: `my-app-app`, `my-app-redis`）
- ネットワーク内のエイリアスとしてコンテナ定義の `name` をそのまま設定（例: `app`, `redis`）

### ネットワークエイリアスの設定

bollard API では `EndpointSettings::aliases` にエイリアスを設定する:

```rust
use bollard::models::EndpointSettings;
use std::collections::HashMap;

// コンテナ作成時の NetworkingConfig でエイリアスを指定
let endpoint_settings = EndpointSettings {
    aliases: Some(config.network_aliases.clone()),
    ..Default::default()
};

let networking_config = NetworkingConfig {
    endpoints_config: HashMap::from([
        (config.network.clone(), endpoint_settings),
    ]),
};
```

これにより、同一ネットワーク内の他コンテナから `app` や `redis` といったコンテナ定義名で名前解決が可能になる。

## コンテナ命名規則

| 項目 | 形式 | 例 |
|---|---|---|
| ネットワーク名 | `egret-<family>` | `egret-my-app` |
| コンテナ名 | `<family>-<container_name>` | `my-app-app` |
| ネットワークエイリアス | `<container_name>` | `app` |

## エラーハンドリング

| エラー状況 | DockerError バリアント | 対応 |
|---|---|---|
| Docker デーモン未起動 | `DaemonNotRunning` | `connect()` で ping 失敗時に検出 |
| イメージが存在しない | `Api(...)` | Docker がデフォルトで pull を試行 |
| ポート競合 | `Api(...)` | Docker API エラーをそのまま伝搬 |
| ネットワーク既存 | — | `create_network` 内で既存を検出し再利用（エラーにしない） |
| コンテナ停止タイムアウト | `Api(...)` | 10 秒後に Docker が自動 kill |

## テスト戦略

Docker API を直接呼ぶ統合テストは CI 環境に依存するため、Phase 1 では以下のアプローチ:

1. **`ContainerConfig` のビルドロジック**: `TaskDefinition` → `ContainerConfig` 変換のユニットテスト（`cli/run.rs` 側）
2. **ラベル・命名規則**: ラベル生成、コンテナ名・ネットワーク名の生成ロジックのユニットテスト
3. **Docker API 呼び出し**: 手動テスト（Docker が利用可能な環境で `cargo run` による確認）

## Phase 1 での制限事項

- イメージの明示的な pull 制御は未実装（Docker のデフォルト動作に委ねる）
- コンテナのリソース制限（CPU/メモリ）は設定するが、厳密な enforcement は Docker に委ねる
- ボリュームマウントは未対応 → Phase 5
- ヘルスチェック設定は未対応 → Phase 4
