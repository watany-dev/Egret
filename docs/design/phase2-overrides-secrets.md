# Phase 2: ローカルオーバーライド + Secrets 差し替え — 設計書

## Context

ECSタスク定義をそのまま使いつつ、ローカル固有の設定（イメージタグ、環境変数、ポートマッピング）を上書きし、Secrets Manager ARN をローカル値に差し替える機能。これにより本番タスク定義を編集せずにローカル実行が可能になる。

---

## 現状把握（origin/main 最新: 893c101）

- `DockerApi` トレイト + `MockDockerClient` 導入済み（テスタブルな設計）
- `main.rs` に `mod secrets;` **宣言済み**（スタブのみ）
- `RunArgs` に `--override` オプション **定義済み**（未使用）
- テストカバレッジ向上: `run_task`, `cleanup` のモックテスト追加済み
- `ContainerDefinition` を直接構築するテスト箇所:
  - `src/cli/run.rs`: `single_container_taskdef()`, `two_container_taskdef()`, `build_container_config_basic()`, `build_container_config_port_default()`, `build_container_config_empty_optionals()`
  - `src/cli/stop.rs`: `ContainerDefinition` 直接構築なし（Docker モックのみ）

---

## 変更対象ファイル

| ファイル | 変更内容 |
|---------|---------|
| `src/taskdef/mod.rs` | `Secret` 構造体追加、`ContainerDefinition` に `secrets` フィールド追加 |
| `src/secrets/mod.rs` | `SecretsResolver` 実装（ARN → ローカル値マッピング） |
| `src/overrides/mod.rs` | **新規作成** — `OverrideConfig` 実装 |
| `src/cli/mod.rs` | `RunArgs` に `--secrets` オプション追加 |
| `src/cli/run.rs` | override + secrets を execute フローに統合 |
| `src/main.rs` | `mod overrides;` 追加（`mod secrets;` は既存） |
| `tests/fixtures/` | テスト用 JSON ファイル追加 |

---

## ファイルフォーマット設計

### `egret-override.json`

```json
{
  "containerOverrides": {
    "nginx": {
      "image": "nginx:1.25-alpine",
      "environment": {
        "NGINX_HOST": "my-local-host",
        "DEBUG": "true"
      },
      "portMappings": [
        { "containerPort": 80, "hostPort": 9090 }
      ]
    },
    "api": {
      "environment": {
        "LOG_LEVEL": "debug"
      }
    }
  }
}
```

**設計根拠**:
- `containerOverrides` はコンテナ名をキーとした map — ECS API の `overrides.containerOverrides` に概念的に対応
- `environment` は flat map (`HashMap<String, String>`) — 手書きの利便性を優先。キーで add/replace のセマンティクスが自然
- `portMappings` は**全置換**（マージしない）— ポートのマージ基準（`containerPort` で一致？追加？）が曖昧なため、全置換がシンプルかつ予測可能
- `image` は文字列でタグ含めて全置換
- 全フィールド optional（部分オーバーライドが主なユースケース）

### `secrets.local.json`

```json
{
  "arn:aws:secretsmanager:us-east-1:123456789:secret:prod/db-password": "local-db-password",
  "arn:aws:secretsmanager:us-east-1:123456789:secret:prod/api-key": "local-api-key"
}
```

- flat `HashMap<String, String>`（ARN → 平文値）
- ECS タスク定義の `secrets` フィールド: `[{"name": "DB_PASSWORD", "valueFrom": "arn:aws:secretsmanager:..."}]`
- 解決: `valueFrom` を mapping で引き、`name` を環境変数名として `KEY=VALUE` 形式で注入

---

## Rust 型定義

### `src/taskdef/mod.rs` への追加

```rust
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

`ContainerDefinition` に追加:
```rust
#[serde(default)]
pub secrets: Vec<Secret>,
```

### `src/secrets/mod.rs`

```rust
#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    #[error("failed to read secrets file from {path}: {source}")]
    ReadFile { path: PathBuf, source: std::io::Error },

    #[error("failed to parse secrets JSON: {0}")]
    ParseJson(#[from] serde_json::Error),

    #[error("secret ARN not found in local mapping: {arn}")]
    ArnNotFound { arn: String },
}

pub struct SecretsResolver {
    mapping: HashMap<String, String>,
}

impl SecretsResolver {
    pub fn from_file(path: &Path) -> Result<Self, SecretsError> { ... }
    pub fn from_json(json: &str) -> Result<Self, SecretsError> { ... }
    pub fn resolve(&self, secrets: &[Secret]) -> Result<Vec<(String, String)>, SecretsError> { ... }
}
```

### `src/overrides/mod.rs` (新規)

```rust
#[derive(Debug, thiserror::Error)]
pub enum OverrideError {
    #[error("failed to read override file from {path}: {source}")]
    ReadFile { path: PathBuf, source: std::io::Error },

    #[error("failed to parse override JSON: {0}")]
    ParseJson(#[from] serde_json::Error),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverrideConfig {
    #[serde(default)]
    pub container_overrides: HashMap<String, ContainerOverride>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerOverride {
    pub image: Option<String>,
    pub environment: Option<HashMap<String, String>>,
    pub port_mappings: Option<Vec<PortMapping>>,  // taskdef::PortMapping を再利用
}

impl OverrideConfig {
    pub fn from_file(path: &Path) -> Result<Self, OverrideError> { ... }
    pub fn from_json(json: &str) -> Result<Self, OverrideError> { ... }
    pub fn apply(&self, task_def: &mut TaskDefinition) { ... }
}
```

---

## データフロー（詳細）

```
1. TaskDefinition::from_file(path)          // secrets フィールドも含めてパース
2. if --override:
     OverrideConfig::from_file(path)
     override_config.apply(&mut task_def)    // image, env, ports を変更
3. if --secrets:
     SecretsResolver::from_file(path)
     for container in &mut task_def.container_definitions:
       resolved = resolver.resolve(&container.secrets)?
       for (name, value) in resolved:
         container.environment.push(Environment { name, value })
4. build_container_config()                  // 変更不要、既存のまま
5. Docker API                                // 変更不要
```

### 優先順位（同名の環境変数がある場合）

1. **task definition の `environment`** が基本
2. **override の `environment`** が `environment` を上書き（Iteration 3 の `apply` で処理）
3. **secrets** が最後に追加（Iteration 4 で `environment` に push）
4. → secrets が override/元の env と同名キーの場合、secrets が勝つ（後勝ち）

この順序は「secrets は本番 ARN の解決なので最も信頼度が高い」という原則に基づく。

### `--secrets` なしだが task definition に `secrets` がある場合

- secrets フィールドは**無視**される（環境変数に変換されない）
- warning を出力: `tracing::warn!("Task definition has secrets but --secrets flag was not provided. Secret values will not be resolved.")`
- エラーにはしない（secrets なしで起動するケースもある）

---

## イテレーション（各イテレーション = 1コミット）

### Iteration 1: `Secret` 型を taskdef に追加

**ファイル**: `src/taskdef/mod.rs`, `src/cli/run.rs`

変更内容:
1. `Secret` 構造体追加（`name`, `value_from`、`#[serde(rename_all = "camelCase")]`）
2. `ContainerDefinition` に `#[serde(default)] pub secrets: Vec<Secret>` 追加
3. 既存テスト修正（`ContainerDefinition` 直接構築箇所に `secrets: vec![]` 追加）:
   - `src/cli/run.rs`: `single_container_taskdef()`, `two_container_taskdef()`, `build_container_config_basic()`, `build_container_config_port_default()`, `build_container_config_empty_optionals()`
4. 新規テスト追加（`src/taskdef/mod.rs`）:
   - `parse_secrets_field`: `secrets` 配列付きの JSON をパースし、`name` と `value_from` が正しいか検証
   - `parse_secrets_empty_default`: `secrets` なしの JSON で `secrets` が空ベクタになることを検証

**make check** で確認。

### Iteration 2: `SecretsResolver` 実装

**ファイル**: `src/secrets/mod.rs`

変更内容:
1. `SecretsError` enum 定義
2. `SecretsResolver` 構造体 + `from_file()`, `from_json()`, `resolve()` 実装
3. テスト:
   - `parse_secrets_mapping`: 正常パース
   - `resolve_all_found`: 全 ARN がマッピングにある場合、正しい `(name, value)` ペアを返す
   - `resolve_missing_arn`: ARN がマッピングにない場合、`ArnNotFound` エラー
   - `resolve_empty_secrets`: 空の secrets リストで空ベクタを返す
   - `error_invalid_json`: 不正 JSON で `ParseJson` エラー
   - `error_file_not_found`: 存在しないファイルで `ReadFile` エラー

**make check** で確認。

### Iteration 3: `OverrideConfig` 実装

**ファイル**: `src/overrides/mod.rs` (新規), `src/main.rs`

変更内容:
1. `OverrideError` enum 定義
2. `OverrideConfig`, `ContainerOverride` 構造体定義
3. `from_file()`, `from_json()` 実装
4. `apply(&self, task_def: &mut TaskDefinition)` 実装:
   - 未知コンテナ名は `tracing::warn!` でスキップ
   - `image`: `Some` なら置換
   - `environment`: キーで検索して既存を上書き、なければ追加
   - `port_mappings`: `Some` なら全置換
5. `src/main.rs` に `mod overrides;` 追加
6. テスト:
   - `parse_full_override`: image + env + ports の完全オーバーライドをパース
   - `parse_empty_override`: `{"containerOverrides": {}}` が正常パース
   - `apply_replaces_image`: image が置換されること
   - `apply_adds_new_env_var`: 新規環境変数が追加されること
   - `apply_replaces_existing_env_var`: 既存環境変数が上書きされること
   - `apply_replaces_port_mappings`: ポートマッピングが全置換されること
   - `apply_unknown_container_skips`: 未知コンテナ名でエラーにならないこと
   - `apply_no_mutation_when_empty`: 空オーバーライドで変更がないこと
   - `error_invalid_json`: 不正 JSON でエラー
   - `error_file_not_found`: 存在しないファイルでエラー

**make check** で確認。

### Iteration 4: CLI 統合

**ファイル**: `src/cli/mod.rs`, `src/cli/run.rs`

変更内容:
1. `RunArgs` に `#[arg(short, long)] pub secrets: Option<PathBuf>` 追加
2. `src/cli/mod.rs` のテスト更新:
   - `parse_run_command` テストに `assert!(args.secrets.is_none())` 追加
   - 新規テスト `parse_run_with_override_and_secrets`: 両オプション指定時のパース検証
3. `execute()` 更新:
   - override ロード・適用
   - secrets ロード・解決・environment への追加
   - secrets ありだが `--secrets` なしの場合の warning
4. テスト用 fixture 追加:
   - `tests/fixtures/task-with-secrets.json`
   - `tests/fixtures/egret-override.json`
   - `tests/fixtures/secrets.local.json`

**make check** で確認。

---

## エラーハンドリング方針

| ケース | 挙動 | 理由 |
|--------|------|------|
| Override ファイル読み込み/パース失敗 | **hard error** | ユーザが明示的に指定したファイル |
| Override に未知コンテナ名 | **warning** + skip | 複数タスク定義で共有するケース |
| Secrets ファイル読み込み/パース失敗 | **hard error** | ユーザが明示的に指定したファイル |
| Secrets ARN がマッピングにない | **hard error** | 実行時にアプリが壊れるため fail-fast |
| タスク定義に secrets あるが `--secrets` なし | **warning** | secrets なしで起動するケースもある |

---

## 検証方法

### 自動テスト
```bash
make check  # fmt-check + lint + test
```

各イテレーションで `make check` を実行し、全チェックが通ることを確認。

### 手動テスト（Docker 環境がある場合）
```bash
# Override テスト
egret run -f tests/fixtures/simple-task.json --override tests/fixtures/egret-override.json

# Secrets テスト
egret run -f tests/fixtures/task-with-secrets.json --secrets tests/fixtures/secrets.local.json

# 両方組み合わせ
egret run -f tests/fixtures/task-with-secrets.json \
  --override tests/fixtures/egret-override.json \
  --secrets tests/fixtures/secrets.local.json
```
