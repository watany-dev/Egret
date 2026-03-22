# Task Definition パーサ設計書

## 概要

ECS タスク定義 JSON をパースし、Lecs 内部の型に変換するモジュール。
`src/taskdef/mod.rs` に実装する。

## 入力仕様

AWS ECS の [RegisterTaskDefinition API](https://docs.aws.amazon.com/AmazonECS/latest/APIReference/API_RegisterTaskDefinition.html) が受け付ける JSON 形式。
Lecs は Phase 1 で必要な主要フィールドのみ対応し、未知フィールドは無視する（`#[serde(deny_unknown_fields)]` は使わない）。

### 技術選定

JSON パーサには `serde` + `serde_json` を使用する。

| 候補 | 判断 | 理由 |
|------|------|------|
| `serde` + `serde_json` | **採用** | Rust のデファクト標準。derive マクロで型安全なパースが可能。`rename_all` で camelCase 対応が容易 |
| `simd-json` | 不採用 | SIMD による高速化が売りだが、タスク定義 JSON は小さいファイル（数KB）のため性能差は無視できる。`unsafe` 依存もあり方針に反する |
| 手動パース (`serde_json::Value`) | 不採用 | 型安全性が低く、フィールド追加時のメンテナンスコストが高い |

### 対応フィールド一覧

```json
{
  "family": "my-app",
  "taskRoleArn": "arn:aws:iam::123456789012:role/my-task-role",
  "executionRoleArn": "arn:aws:iam::123456789012:role/my-execution-role",
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
      "secrets": [
        { "name": "DB_PASSWORD", "valueFrom": "arn:aws:secretsmanager:us-east-1:123456789:secret:prod/db-password" }
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

    /// タスク IAM ロール ARN（Phase 3 で追加）
    #[serde(default)]
    pub task_role_arn: Option<String>,

    /// 実行 IAM ロール ARN（Phase 3 で追加）
    #[serde(default)]
    pub execution_role_arn: Option<String>,

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

    /// Secrets Manager 参照
    #[serde(default)]
    pub secrets: Vec<Secret>,

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

/// Secret reference (Secrets Manager ARN).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Secret {
    /// Environment variable name to inject.
    pub name: String,
    /// ARN of the secret in Secrets Manager.
    pub value_from: String,
}
```

## エラー型

`thiserror` でモジュール専用のエラー型を定義する。CLI 層で `anyhow` に変換する。

```rust
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum TaskDefError {
    /// ファイル読み込みエラー
    #[error("failed to read task definition from {path}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    /// JSON パースエラー
    #[error("failed to parse task definition JSON: {0}")]
    ParseJson(#[from] serde_json::Error),

    /// バリデーションエラー
    #[error("task definition validation failed: {0}")]
    Validation(String),
}
```

## 公開 API

```rust
impl TaskDefinition {
    /// ファイルパスから task definition を読み込む
    pub fn from_file(path: &Path) -> Result<Self, TaskDefError>;

    /// JSON 文字列からパースする（テスト用にも利用）
    pub fn from_json(json: &str) -> Result<Self, TaskDefError>;

    /// バリデーションを実行する
    /// from_file / from_json 内でパース後に自動呼び出しされる
    fn validate(&self) -> Result<(), TaskDefError>;
}
```

## データフロー

```
ファイルパス (&Path)
    │
    ▼
┌──────────────────┐
│ std::fs::read_to_string()                │
│ エラー → TaskDefError::ReadFile          │
└────────┬─────────┘
         │ JSON 文字列
         ▼
┌──────────────────┐
│ serde_json::from_str::<TaskDefinition>() │
│ エラー → TaskDefError::ParseJson         │
└────────┬─────────┘
         │ TaskDefinition（未検証）
         ▼
┌──────────────────┐
│ validate()                               │
│ エラー → TaskDefError::Validation        │
└────────┬─────────┘
         │ TaskDefinition（検証済み）
         ▼
    Ok(TaskDefinition)
```

## バリデーション仕様

最初のエラーで即座に `Err` を返す（fail-fast 方式）。

| ルール | エラーメッセージ |
|--------|----------------|
| `family` が空文字列 | `"family must not be empty"` |
| `container_definitions` が空配列 | `"containerDefinitions must not be empty"` |
| いずれかのコンテナの `name` が空 | `"container name must not be empty at index {i}"` |
| いずれかのコンテナの `image` が空 | `"container image must not be empty for container '{name}'"` |

## テスト戦略

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // 1. 正常系: 全フィールド指定の JSON をパースできる
    // 2. 正常系: オプションフィールド省略の最小 JSON をパースできる
    // 3. 正常系: 未知フィールドが含まれていても無視してパースできる
    // 4. 異常系: family が空 → TaskDefError::Validation
    // 5. 異常系: containerDefinitions が空配列 → TaskDefError::Validation
    // 6. 異常系: 不正な JSON → TaskDefError::ParseJson
    // 7. 異常系: 必須フィールド欠落 → TaskDefError::ParseJson
    // 8. デフォルト値: essential=true, protocol="tcp", command=[], entry_point=[]
}
```

## 実装済みフィールド（後続フェーズで追加）

- `secrets` / `valueFrom` — Phase 2 で実装済み（`Secret` 構造体）
- `taskRoleArn` / `executionRoleArn` — Phase 3 で実装済み（メタデータレスポンスで使用）

## Phase 4 で追加されたフィールド

- `healthCheck` — ヘルスチェック設定（`command`, `interval`, `timeout`, `retries`, `startPeriod`）
- `dependsOn` — コンテナ依存関係（`containerName`, `condition`）
- `DependencyCondition` 列挙型 — `START`, `COMPLETE`, `SUCCESS`, `HEALTHY`

バリデーション追加:
- 自己参照 dependsOn の検出
- 存在しないコンテナ名への参照検出
- `HEALTHY` 条件で `healthCheck` 未設定の検出

## 未対応フィールド

以下のフィールドは後続フェーズで追加予定:

- `volumes` / `mountPoints` → Phase 5
- `logConfiguration` → Phase 5
- `links`, `volumesFrom`, `dockerLabels`, `ulimits` 等 → 未定
