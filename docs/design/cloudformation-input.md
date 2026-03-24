# CloudFormation / CDK テンプレート入力対応 設計書

## 概要

CloudFormation テンプレート JSON（CDK `cdk synth` 出力を含む）から `AWS::ECS::TaskDefinition` リソースを抽出し、既存の `TaskDefinition` 型に変換する。これにより、CloudFormation や CDK で管理された ECS タスク定義を直接 `lecs run` / `lecs validate` / `lecs watch` で利用可能にする。

## 背景

CDK や CloudFormation を使用して ECS タスク定義を管理するチームでは、タスク定義が CloudFormation テンプレート内に埋め込まれている。CDK は `cdk synth` で CloudFormation テンプレート JSON を生成するため、CloudFormation テンプレートパーサーがあれば CDK も自動的にカバーできる。

## 技術選定

### パース戦略: PascalCase → camelCase キー変換

CloudFormation テンプレートではプロパティキーが **PascalCase** で記述される。既存の `TaskDefinition` 型は `#[serde(rename_all = "camelCase")]` で定義されているため、キーを変換してから既存のデシリアライゼーションを再利用する。

```
CloudFormation Properties (PascalCase)
  └── serde_json::Value として読み込み
        └── キー名を再帰的に camelCase に変換
              └── serde_json::from_value::<TaskDefinition>() で既存型にデシリアライズ
```

新しい serde 型を定義する代わりにキー変換を使うことで:
- 既存の `TaskDefinition` 型を変更せずに済む
- フィールド追加時に cloudformation.rs の同期が不要
- Terraform パーサーの二重デシリアライゼーションと同様の戦略

### Intrinsic Function の扱い

CloudFormation の Intrinsic Function（`Ref`, `Fn::Sub`, `Fn::Join` 等）はローカルでは解決できないため、検出時にエラーを返す。CDK `cdk synth` の出力は完全に解決されたテンプレートなので、通常 Intrinsic Function を含まない。

検出はキー変換の**前**に行う。これにより、ユーザーデータ内の camelCase キーとの混同を防ぐ。

### YAML 対応（将来）

初回リリースは JSON のみ対応。YAML 対応は新規依存（`serde_yml`）の追加が必要で、`!Ref` 等のカスタムタグ処理も複雑なため、将来対応とする。

## モジュール構成

```
src/taskdef/
├── mod.rs              # TaskDefinition, TaskDefError (CFn関連バリアントを追加)
├── cloudformation.rs   # CloudFormation テンプレートパーサ (新規)
├── terraform.rs        # Terraform JSON パーサ (変更なし)
└── diagnostics.rs      # バリデーション診断 (変更なし)
```

## 型定義

### CloudFormation テンプレートスキーマ型

```rust
/// CloudFormation テンプレートのトップレベル構造
struct CfnTemplate {
    resources: Option<HashMap<String, CfnResource>>,
}

/// CloudFormation リソース
struct CfnResource {
    resource_type: String,      // e.g. "AWS::ECS::TaskDefinition"
    properties: Option<Value>,  // PascalCase の生 JSON
}
```

### エラーバリアント（TaskDefError に追加）

```rust
pub enum TaskDefError {
    // ... 既存バリアント ...

    CfnNoEcsResource,
    CfnMultipleResources { resources: Vec<String> },
    CfnResourceNotFound(String),
    ParseCfnJson(String),
    CfnIntrinsicFunction { field: String, detail: String },
}
```

## 公開 API

```rust
/// CloudFormation テンプレートファイルから ECS タスク定義を抽出
pub fn from_cfn_file(
    path: &Path,
    resource_id: Option<&str>,
) -> Result<TaskDefinition, TaskDefError>;

/// CloudFormation テンプレート JSON 文字列から ECS タスク定義を抽出
pub fn from_cfn_json(
    json: &str,
    resource_id: Option<&str>,
) -> Result<TaskDefinition, TaskDefError>;
```

## リソース選択ロジック

| 状況 | 動作 |
|------|------|
| ECS リソースが 1 つ | 自動選択 |
| ECS リソースが複数 + `--cfn-resource` なし | `CfnMultipleResources` エラー |
| ECS リソースが複数 + `--cfn-resource` あり | 論理IDで選択 |
| `--cfn-resource` のIDが見つからない | `CfnResourceNotFound` エラー |
| ECS リソースが 0 | `CfnNoEcsResource` エラー |

## データフロー

```
lecs run --from-cfn template.json [--cfn-resource MyTaskDef]
  │
  ├── from_cfn_file(path, resource_id)
  │     ├── ファイルサイズチェック (10MB)
  │     ├── JSON パース → CfnTemplate
  │     ├── Resources から AWS::ECS::TaskDefinition を収集
  │     ├── リソース選択 (単一 or --cfn-resource)
  │     ├── Intrinsic Function 検出 (エラー)
  │     ├── PascalCase → camelCase キー変換
  │     ├── serde_json::from_value::<TaskDefinition>()
  │     └── validate()
  │
  ├── オーバーライド適用 (--override)
  ├── Secrets 解決 (--secrets)
  └── コンテナ起動
```

## CLI フラグ

### 対応コマンド

| コマンド | `--from-cfn` | `--cfn-resource` |
|----------|:------------:|:----------------:|
| `lecs run` | ✅ | ✅ |
| `lecs validate` | ✅ | ✅ |
| `lecs watch` | ✅ | ✅ |

### フラグ設計

```rust
/// CloudFormation テンプレート JSON ファイルのパス（-f, --from-tf と排他）
#[arg(long = "from-cfn", conflicts_with_all = ["task_definition", "from_tf"])]
pub from_cfn: Option<PathBuf>,

/// 複数 ECS リソースがある場合の論理リソースID指定
#[arg(long = "cfn-resource", requires = "from_cfn")]
pub cfn_resource: Option<String>,
```

## 制約・制限事項

- **JSON のみ対応**: YAML テンプレートは未サポート（将来対応）
- **Intrinsic Function 非対応**: `Ref`, `Fn::Sub` 等を含むテンプレートはエラー（解決済みテンプレートを想定）
- **CDK ディレクトリ探索なし**: `cdk.out/` の自動探索は未サポート（`*.template.json` を直接指定）
- **ファイルサイズ上限**: 10 MB
- **PascalCase 変換**: 先頭1文字のみ小文字化（ECS TaskDefinition の全フィールドに対応）

## テスト戦略

### ユニットテスト（`cloudformation.rs` 内）

| テストケース | 検証内容 |
|-------------|---------|
| `parse_single_resource` | 単一リソースの正常パース |
| `parse_minimal_template` | 最小構成テンプレート |
| `parse_with_volumes` | Volume の PascalCase 変換 |
| `parse_with_health_check` | HealthCheck の変換 |
| `parse_with_depends_on` | DependsOn の変換 |
| `parse_with_secrets` | Secrets の変換 |
| `parse_with_port_mappings` | PortMappings の変換 |
| `select_resource_by_id` | 複数リソース + `--cfn-resource` での選択 |
| `error_multiple_resources` | 複数リソース + セレクタなしのエラー |
| `error_no_ecs_resource` | ECS リソースなしのエラー |
| `error_resource_not_found` | 指定リソースが見つからないエラー |
| `error_intrinsic_ref` | `Ref` 検出時のエラー |
| `error_intrinsic_fn_sub` | `Fn::Sub` 検出時のエラー |
| `error_intrinsic_fn_join` | `Fn::Join` 検出時のエラー |
| `error_empty_resources` | `Resources` が空のエラー |
| `error_invalid_json` | 不正 JSON のエラー |
| `error_file_too_large` | ファイルサイズ超過のエラー |
| `pascal_to_camel_conversion` | キー名変換の単体テスト |
| `ignore_unknown_resource_types` | 非 ECS リソースのスキップ |

### CLI テスト（`cli/mod.rs` 内）

| テストケース | 検証内容 |
|-------------|---------|
| `parse_run_with_from_cfn` | `--from-cfn` フラグのパース |
| `parse_run_from_cfn_with_resource` | `--from-cfn` + `--cfn-resource` のパース |
| `parse_run_cfn_conflicts_with_tf` | `--from-cfn` と `--from-tf` の排他制御 |
| `parse_run_cfn_conflicts_with_f` | `--from-cfn` と `-f` の排他制御 |
| `parse_validate_with_from_cfn` | validate での `--from-cfn` |
| `parse_watch_with_from_cfn` | watch での `--from-cfn` |
