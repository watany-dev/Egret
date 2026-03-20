# Docker クライアント設計書

## 概要

bollard クレートを通じて Docker Engine API と連携するモジュール。
コンテナ・ネットワークの作成・起動・停止・削除およびログストリームを提供する。
`src/docker/mod.rs` に実装する。

## 設計方針

- bollard の API を薄くラップし、Egret 固有のロジック（ラベル管理、命名規則）をカプセル化
- すべての操作は `async` で提供
- Docker デーモンへの接続はデフォルトの Unix ソケット（`/var/run/docker.sock`）を使用

## ラベル戦略

Egret が管理するリソースを識別するために、Docker ラベルを付与する:

| ラベルキー | 値 | 用途 |
|---|---|---|
| `egret.managed` | `"true"` | Egret が作成したリソースの識別 |
| `egret.task` | `<family>` | タスクファミリー名 |
| `egret.container` | `<container-name>` | コンテナ定義名 |

ネットワークにも同じラベルを付与する（`egret.managed`, `egret.task`）。

## 型定義

```rust
use bollard::Docker;
use anyhow::Result;

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
    /// ポートマッピング（`host_port:container_port/protocol`）
    pub port_mappings: Vec<PortMappingConfig>,
    /// 接続するネットワーク名
    pub network: String,
    /// Docker ラベル
    pub labels: HashMap<String, String>,
}

pub struct PortMappingConfig {
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: String,
}
```

## 公開 API

```rust
impl DockerClient {
    /// Docker デーモンに接続し、接続確認（ping）を行う
    pub async fn connect() -> Result<Self>;

    // --- ネットワーク ---

    /// Egret 専用ネットワークを作成する
    /// ネットワーク名: `egret-<family>`
    /// ドライバ: bridge（コンテナ名での DNS 解決が有効）
    pub async fn create_network(
        &self,
        family: &str,
    ) -> Result<String>;

    /// ネットワークを削除する
    pub async fn remove_network(&self, name: &str) -> Result<()>;

    /// Egret が管理するネットワーク一覧を取得する
    pub async fn list_networks(
        &self,
        task_filter: Option<&str>,
    ) -> Result<Vec<NetworkInfo>>;

    // --- コンテナ ---

    /// コンテナを作成する（起動はしない）
    pub async fn create_container(
        &self,
        config: &ContainerConfig,
    ) -> Result<String>;

    /// コンテナを起動する
    pub async fn start_container(&self, id: &str) -> Result<()>;

    /// コンテナを停止する（タイムアウト: 10秒）
    pub async fn stop_container(&self, id: &str) -> Result<()>;

    /// コンテナを削除する
    pub async fn remove_container(&self, id: &str) -> Result<()>;

    /// Egret が管理するコンテナ一覧を取得する
    pub async fn list_containers(
        &self,
        task_filter: Option<&str>,
    ) -> Result<Vec<ContainerInfo>>;

    // --- ログ ---

    /// コンテナのログをストリームとして返す
    /// follow=true でリアルタイムストリーム
    pub async fn stream_logs(
        &self,
        id: &str,
    ) -> Result<impl futures_util::Stream<Item = Result<String>>>;
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

## コンテナ命名規則

| 項目 | 形式 | 例 |
|---|---|---|
| ネットワーク名 | `egret-<family>` | `egret-my-app` |
| コンテナ名 | `<family>-<container_name>` | `my-app-app` |
| ネットワークエイリアス | `<container_name>` | `app` |

## エラーハンドリング

| エラー状況 | 対応 |
|---|---|
| Docker デーモン未起動 | `connect()` で検出、ユーザーフレンドリーなメッセージ |
| イメージが存在しない | bollard が pull を試行（デフォルト動作） |
| ポート競合 | Docker API エラーをそのまま伝搬 |
| ネットワーク既存 | 既存ネットワークを再利用（エラーにしない） |
| コンテナ停止タイムアウト | 10 秒後に強制停止（kill） |

## テスト戦略

Docker API を直接呼ぶ統合テストは CI 環境に依存するため、Phase 1 では以下のアプローチ:

1. **`ContainerConfig` のビルドロジック**: `TaskDefinition` → `ContainerConfig` 変換のユニットテスト
2. **ラベル・命名規則**: ラベル生成、コンテナ名・ネットワーク名の生成ロジックのユニットテスト
3. **Docker API 呼び出し**: 手動テスト（Docker が利用可能な環境で `cargo run` による確認）

## 依存クレート追加

```toml
[dependencies]
bollard = "0.18"
futures-util = "0.3"
```

## Phase 1 での制限事項

- イメージの明示的な pull 制御は未実装（Docker のデフォルト動作に委ねる）
- コンテナのリソース制限（CPU/メモリ）は設定するが、厳密な enforcement は Docker に委ねる
- ボリュームマウントは未対応 → Phase 5
- ヘルスチェック設定は未対応 → Phase 4
