# Terraform Plan/State 入力対応 設計書

## 概要

`terraform show -json` の出力（Plan または State）から `aws_ecs_task_definition` リソースを抽出し、既存の `TaskDefinition` 型に変換する。これにより、Terraform で管理された ECS タスク定義を直接 `lecs run` / `lecs validate` / `lecs watch` で利用可能にする。

## 背景

Terraform を使用して ECS タスク定義を管理するチームでは、タスク定義 JSON が Terraform コード内に埋め込まれており、単体の JSON ファイルとして存在しない。`terraform show -json` を使えば計画済みの状態を JSON として取得できるが、その構造は ECS タスク定義 JSON とは大きく異なる。

## 技術選定

### パース戦略: 二重デシリアライゼーション

Terraform 出力では `container_definitions` が **JSON エンコードされた文字列**として格納される。内部の JSON は ECS API 互換の **camelCase** 形式。

```
Terraform JSON (snake_case envelope)
  └── container_definitions: "{\"name\":\"app\",\"image\":\"nginx\"}"  ← JSON文字列
        └── 内部JSON (camelCase) → serde で ContainerDefinition[] へ
```

これにより、既存の `ContainerDefinition` の `#[serde(rename_all = "camelCase")]` をそのまま活用できる。

## モジュール構成

```
src/taskdef/
├── mod.rs          # TaskDefinition, TaskDefError (Terraform関連バリアントを追加)
├── terraform.rs    # Terraform JSON パーサ (新規)
└── diagnostics.rs  # バリデーション診断 (変更なし)
```

`taskdef` の子モジュールとして配置。Rust のビジョイビリティルールにより、子モジュールは親の private アイテム（`validate()` メソッド等）にアクセス可能。

## 型定義

### Terraform JSON スキーマ型

```rust
/// `terraform show -json` のトップレベル構造
struct TerraformShowJson {
    planned_values: Option<PlannedValues>,  // Plan 出力
    values: Option<StateValues>,            // State 出力
    resource_changes: Option<Vec<ResourceChange>>,  // Plan 出力（フォールバック）
}

/// Terraform モジュール（再帰的に子モジュールを含む）
struct Module {
    resources: Vec<Resource>,
    child_modules: Vec<Module>,
}

/// Terraform リソース
struct Resource {
    address: String,       // e.g. "module.ecs.aws_ecs_task_definition.app"
    resource_type: String, // e.g. "aws_ecs_task_definition"
    values: serde_json::Value,
}

/// resource_changes のエントリ
struct ResourceChange {
    address: String,
    resource_type: String,
    change: Change,
}

/// 変更ブロック（after が None の場合は destroy アクション）
struct Change {
    after: Option<serde_json::Value>,
}

/// Terraform の volume ブロック（snake_case、単数形 "volume"）
struct TerraformVolume {
    name: String,
    host_path: Option<String>,
}
```

### エラーバリアント（TaskDefError に追加）

```rust
pub enum TaskDefError {
    // ... 既存バリアント ...

    #[error("no aws_ecs_task_definition resource found in Terraform JSON")]
    TerraformNoEcsResource,

    #[error("multiple aws_ecs_task_definition resources found: {resources:?}. Use --tf-resource to specify one")]
    TerraformMultipleResources { resources: Vec<String> },

    #[error("terraform resource '{0}' not found")]
    TerraformResourceNotFound(String),

    #[error("failed to parse Terraform JSON: {0}")]
    ParseTerraformJson(String),
}
```

## 公開 API

```rust
/// Terraform show -json ファイルから ECS タスク定義を抽出
pub fn from_terraform_file(
    path: &Path,
    resource_address: Option<&str>,
) -> Result<TaskDefinition, TaskDefError>;

/// Terraform show -json 文字列から ECS タスク定義を抽出
pub fn from_terraform_json(
    json: &str,
    resource_address: Option<&str>,
) -> Result<TaskDefinition, TaskDefError>;
```

## リソース収集の優先順位

Terraform の出力形式に応じて、以下の優先順位で ECS リソースを探索する:

1. `planned_values.root_module` — Plan 出力の主要パス
2. `values.root_module` — State 出力のパス
3. `resource_changes[].change.after` — Plan 出力のフォールバック

各パスで再帰的に子モジュールも探索する（`child_modules`）。

## リソース選択ロジック

| 状況 | 動作 |
|------|------|
| ECS リソースが 1 つ | 自動選択 |
| ECS リソースが複数 + `--tf-resource` なし | `TerraformMultipleResources` エラー |
| ECS リソースが複数 + `--tf-resource` あり | 指定アドレスと一致するリソースを選択 |
| `--tf-resource` のアドレスが見つからない | `TerraformResourceNotFound` エラー |
| ECS リソースが 0 | `TerraformNoEcsResource` エラー |
| destroy アクション（`after: null`）| スキップ（resource_changes のみ） |

## 変換ロジック

### container_definitions の二重デシリアライゼーション

```
Terraform values.container_definitions (文字列)
  → serde_json::from_str::<Vec<ContainerDefinition>>()
    → 既存の ContainerDefinition 型（camelCase）で直接デシリアライズ
```

### Volume 変換

| Terraform (snake_case) | Lecs (camelCase) |
|------------------------|------------------|
| `volume[].name` | `volumes[].name` |
| `volume[].host_path` | `volumes[].host.sourcePath` |

EFS/Docker volume 設定は未サポート（`host_path` のない volume は `host: None` として変換）。

### バリデーション

変換後の `TaskDefinition` に対して既存の `validate()` メソッドを実行。これにより ECS タスク定義と同じバリデーションルールが適用される。

## CLI フラグ

### 対応コマンド

| コマンド | `--from-tf` | `--tf-resource` |
|----------|:-----------:|:---------------:|
| `lecs run` | ✅ | ✅ |
| `lecs validate` | ✅ | ✅ |
| `lecs watch` | ✅ | ✅ |
| `lecs diff` | — | — |

### フラグ設計

```rust
/// Terraform show JSON ファイルのパス（-f/--task-definition と排他）
#[arg(long = "from-tf", conflicts_with = "task_definition")]
pub from_tf: Option<PathBuf>,

/// 複数 ECS リソースがある場合のリソースアドレス指定
#[arg(long = "tf-resource", requires = "from_tf")]
pub tf_resource: Option<String>,
```

- `--from-tf` と `-f/--task-definition` は `conflicts_with` で排他制御
- `--tf-resource` は `requires = "from_tf"` で `--from-tf` を必須化
- いずれかが必須: `required_unless_present = "from_tf"` を `-f` に設定

## データフロー

```
terraform show -json tfplan > plan.json

lecs run --from-tf plan.json [--tf-resource aws_ecs_task_definition.app]
  │
  ├── from_terraform_file(path, resource_address)
  │     ├── ファイルサイズチェック (10MB)
  │     ├── JSON パース → TerraformShowJson
  │     ├── ECS リソース収集 (planned_values → values → resource_changes)
  │     ├── リソース選択 (単一 or --tf-resource)
  │     ├── container_definitions 二重デシリアライゼーション
  │     ├── volume 変換 (snake_case → camelCase)
  │     └── validate()
  │
  ├── オーバーライド適用 (--override)
  ├── Secrets 解決 (--secrets)
  └── コンテナ起動
```

## 制約・制限事項

- **`terraform show -json` のみ対応**: `terraform plan -json` のストリーミング形式（1行1JSON）は非対応
- **Bind mount のみ**: EFS volume、Docker volume は `host: None` として変換（`host_path` のみサポート）
- **フォーマット自動検出なし**: `--from-tf` フラグによる明示的指定が必要
- **ファイルサイズ上限**: 10 MB（大規模 Terraform Plan に対応）

## テスト戦略

### ユニットテスト（`terraform.rs` 内）

| テストケース | 検証内容 |
|-------------|---------|
| `parse_single_resource_from_plan` | Plan 出力から単一リソースの正常パース |
| `parse_volumes_from_plan` | Volume の変換（host_path あり/なし） |
| `parse_with_tf_resource_selection` | 複数リソース + `--tf-resource` での選択 |
| `error_multiple_resources_without_selector` | 複数リソース + セレクタなしのエラー |
| `error_no_ecs_resource` | ECS リソースなしのエラー |
| `error_resource_not_found` | 指定リソースが見つからないエラー |
| `error_invalid_container_definitions_json` | container_definitions の不正 JSON |
| `error_invalid_terraform_json` | Terraform JSON 自体の不正 |
| `parse_state_output` | State 出力からのパース |
| `parse_resource_changes_fallback` | resource_changes フォールバック |
| `skip_destroy_in_resource_changes` | destroy アクションのスキップ |
| `parse_child_modules` | 子モジュール内のリソース探索 |
| `volume_without_host_path` | host_path なし volume の変換 |
| `missing_family_field` | family フィールド欠損のエラー |
| `missing_container_definitions_field` | container_definitions フィールド欠損のエラー |

### CLI テスト（`cli/mod.rs` 内）

| テストケース | 検証内容 |
|-------------|---------|
| `parse_run_with_from_tf` | `--from-tf` フラグのパース |
| `parse_run_with_from_tf_and_resource` | `--from-tf` + `--tf-resource` のパース |
| `parse_run_from_tf_conflicts_with_task_definition` | `-f` と `--from-tf` の排他制御 |
| `parse_run_requires_either_f_or_from_tf` | いずれかの指定が必須 |
| `parse_run_tf_resource_requires_from_tf` | `--tf-resource` は `--from-tf` 必須 |
| `parse_validate_with_from_tf` | validate での `--from-tf` |
| `parse_watch_with_from_tf` | watch での `--from-tf` |
