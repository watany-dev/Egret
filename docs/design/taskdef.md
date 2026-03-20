# Task Definition パーサ設計書

## 概要

ECS タスク定義 JSON をパースし、Egret 内部の型に変換するモジュール。
`src/taskdef/mod.rs` に実装する。

## 入力仕様

AWS ECS の [RegisterTaskDefinition API](https://docs.aws.amazon.com/AmazonECS/latest/APIReference/API_RegisterTaskDefinition.html) が受け付ける JSON 形式。
Egret は Phase 1 で必要な主要フィールドのみ対応し、未知フィールドは無視する（`#[serde(deny_unknown_fields)]` は使わない）。

### 対応フィールド一覧

```json
{
  "family": "my-app",
  "containerDefinitions": [
    {
      "name": "app",
      "image": "nginx:latest",
      "essential": true,
      "command": ["nginx", "-g", "daemon off;"],
      "entryPoint": ["/docker-entrypoint.sh"],
      "environment": [
        { "name": "ENV_VAR", "value": "some-value" }
      ],
      "portMappings": [
        { "containerPort": 80, "hostPort": 8080, "protocol": "tcp" }
      ],
      "cpu": 256,
      "memory": 512,
      "memoryReservation": 256
    }
  ]
}
```

## 型定義

```rust
use serde::Deserialize;
use std::path::Path;

/// ECS タスク定義のトップレベル構造体
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDefinition {
    /// タスクファミリー名（ネットワーク名・ラベルに使用）
    pub family: String,

    /// コンテナ定義の配列
    pub container_definitions: Vec<ContainerDefinition>,
}

/// 個別コンテナの定義
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerDefinition {
    /// コンテナ名（Docker コンテナ名・DNS 名に使用）
    pub name: String,

    /// Docker イメージ
    pub image: String,

    /// essential フラグ（デフォルト: true）
    #[serde(default = "default_essential")]
    pub essential: bool,

    /// CMD に相当
    #[serde(default)]
    pub command: Vec<String>,

    /// ENTRYPOINT に相当
    #[serde(default)]
    pub entry_point: Vec<String>,

    /// 環境変数
    #[serde(default)]
    pub environment: Vec<Environment>,

    /// ポートマッピング
    #[serde(default)]
    pub port_mappings: Vec<PortMapping>,

    /// CPU ユニット（1024 = 1 vCPU）
    pub cpu: Option<u32>,

    /// ハードメモリ制限（MiB）
    pub memory: Option<u32>,

    /// ソフトメモリ制限（MiB）
    pub memory_reservation: Option<u32>,
}

fn default_essential() -> bool {
    true
}

/// 環境変数の名前-値ペア
#[derive(Debug, Clone, Deserialize)]
pub struct Environment {
    pub name: String,
    pub value: String,
}

/// ポートマッピング
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortMapping {
    /// コンテナ側ポート
    pub container_port: u16,

    /// ホスト側ポート（省略時はコンテナポートと同じ）
    pub host_port: Option<u16>,

    /// プロトコル（デフォルト: "tcp"）
    #[serde(default = "default_protocol")]
    pub protocol: String,
}

fn default_protocol() -> String {
    "tcp".to_string()
}
```

## 公開 API

```rust
impl TaskDefinition {
    /// ファイルパスから task definition を読み込む
    pub fn from_file(path: &Path) -> anyhow::Result<Self>;

    /// JSON 文字列からパースする（テスト用にも利用）
    pub fn from_json(json: &str) -> anyhow::Result<Self>;
}
```

## エラーハンドリング

- ファイルが存在しない → `anyhow` でファイルパス付きエラーメッセージ
- JSON パースエラー → `serde_json::Error` をそのまま `anyhow` で伝搬
- `family` が空文字列 → バリデーションエラー
- `containerDefinitions` が空配列 → バリデーションエラー
- `name` または `image` が空文字列 → バリデーションエラー

バリデーションは `TaskDefinition::validate(&self) -> anyhow::Result<()>` として分離し、
`from_file` / `from_json` 内でパース後に自動呼び出しする。

## テスト戦略

```rust
#[cfg(test)]
mod tests {
    // 1. 正常系: 全フィールド指定の JSON をパースできる
    // 2. 正常系: オプションフィールド省略の最小 JSON をパースできる
    // 3. 正常系: 未知フィールドが含まれていても無視してパースできる
    // 4. 異常系: family が空 → エラー
    // 5. 異常系: containerDefinitions が空配列 → エラー
    // 6. 異常系: 不正な JSON → エラー
    // 7. 異常系: 必須フィールド欠落 → エラー
    // 8. デフォルト値: essential=true, protocol="tcp", command=[]
}
```

## Phase 1 での制限事項

以下のフィールドは Phase 1 では未対応。後続フェーズで追加予定:

- `secrets` / `valueFrom` → Phase 2
- `healthCheck` → Phase 4
- `dependsOn` → Phase 4
- `volumes` / `mountPoints` → Phase 5
- `logConfiguration` → Phase 5
- `links`, `volumesFrom`, `dockerLabels`, `ulimits` 等 → 未定
