# Phase 13: CloudFormation YAML対応 + ネットワークモード拡張

## 概要

Phase 13 では2つの機能を追加した:

1. **CloudFormation YAML対応 + CDK自動探索**: YAML 形式サポートと `--from-cdk` によるディレクトリ自動探索
2. **ネットワークモード拡張**: `networkMode: host` / `none` のサポート（awsvpc は bridge エイリアス）

---

## Part A: CloudFormation YAML + CDK自動探索

### 技術選定

| クレート | 状態 | 判定 |
|---------|------|------|
| `serde_yaml` 0.9.34 | 2024/3 deprecated, archived | 使用不可 |
| `serde_yml` 0.0.12 | archived, unsoundness報告あり | 使用不可 |
| **`serde_yaml_ng`** 0.9.35+ | 活発にメンテ、serde_yaml互換API | **採用** |
| `serde-saphyr` 0.0.10 | 高性能、異なるAPI | API非互換 |
| `yaml-rust2` | 低レベルパーサ、Serde非統合 | 統合コスト大 |

`serde_yaml_ng` は `serde_yaml` のドロップイン互換フォークで、MIT ライセンス（`deny.toml` 許可リスト内）。

### YAML パースフロー

```
YAML文字列
  → serde_yaml_ng::from_str::<serde_json::Value>()
  → serde_json::from_value::<CfnTemplate>()
  → collect_ecs_resources()
  → select_resource()
  → detect_intrinsic_functions()
  → convert_keys_to_camel_case()
  → TaskDefinition デシリアライズ
  → validate()
```

### フォーマット自動検出

`from_cfn_file()` でファイル拡張子に基づき判定:
- `.yaml` / `.yml` → YAML パーサ
- `.json` → JSON パーサ
- 不明 → JSON を試行、失敗時に YAML フォールバック

### YAML カスタムタグの扱い

CloudFormation の `!Ref`, `!Sub` 等はカスタムタグ。`serde_yaml_ng` はこれを `Value::Tagged` 型で表現するが、`serde_json::Value` への変換時にエラーとなる。これは意図的な動作 — Intrinsic Function を含むテンプレートは `ParseCfnYaml` エラーとして報告される。

### CDK 自動探索

`discover_cdk_template()` のフロー:
1. 指定ディレクトリ内の `*.template.json` を列挙
2. 単一テンプレート → 即採用
3. 複数テンプレート → ECS リソースを持つテンプレートに絞り込み
4. `--cdk-resource` 指定時は論理 ID で明示選択

### エラーバリアント

```rust
// src/taskdef/mod.rs に追加
ParseCfnYaml(String),
CdkDirectoryNotFound { path: String },
CdkNoTemplatesFound { path: String },
CdkNoEcsResourcesFound { path: String },
CdkResourceNotFound { resource_id: String, available: Vec<String> },
CdkMultipleResources { candidates: Vec<String> },
```

### CLI フラグ

```
--from-cdk <DIR>           CDK 出力ディレクトリ (cdk.out/)
--cdk-resource <ID>        CDK テンプレート内の論理リソースID
```

`--from-cdk` は `--task-definition`, `--from-tf`, `--from-cfn` と排他。

---

## Part B: ネットワークモード拡張

### NetworkMode enum

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkMode {
    #[default]
    Bridge,
    Host,
    None,
    Awsvpc,
}
```

- `effective()`: `Awsvpc` → `Bridge` に変換（ローカルでは同等）
- `as_str()`: 文字列表現を返す

### モード別動作

| 機能 | Bridge | Host | None |
|------|--------|------|------|
| Docker ネットワーク作成 | `lecs-<family>` | スキップ | スキップ |
| NetworkingConfig | エイリアス付き | なし | なし |
| HostConfig.network_mode | なし (デフォルト) | `"host"` | `"none"` |
| メタデータ URI | `host.docker.internal:<port>` | `127.0.0.1:<port>` | 注入しない |
| extra_hosts | `host.docker.internal:host-gateway` | なし | なし |
| メタデータサーバー | 起動 | 起動 | 起動しない |
| ネットワーク削除 (cleanup) | 実行 | スキップ | スキップ |

### awsvpc の扱い

`awsvpc` はローカルでは bridge として動作する。検出時に `tracing::warn!` で通知し、`effective()` で `Bridge` に変換。バリデーションでも Warning レベル診断を出力。

### host モードのポート制約

ECS の host モードでは `hostPort == containerPort` が強制される。`hostPort` が `containerPort` と異なる場合、バリデーション警告を出力。

### 影響ファイル

| ファイル | 変更内容 |
|---------|---------|
| `src/taskdef/mod.rs` | `NetworkMode` enum、`TaskDefinition.network_mode` フィールド |
| `src/taskdef/diagnostics.rs` | `check_network_mode()` バリデーション |
| `src/container/mod.rs` | `ContainerConfig.network_mode`、`build_bollard_config()` 分岐 |
| `src/cli/task_lifecycle.rs` | ネットワーク作成スキップ、メタデータ URI 切替、extra_hosts 調整 |
| `src/metadata/mod.rs` | `build_container_metadata()` の network_mode パラメータ化 |
| `src/orchestrator/mod.rs` | クリーンアップ時のネットワーク削除スキップ |

---

## テスト

- YAML パース: 16 テスト（最小テンプレート、volumes 付き、intrinsic 検出、フォーマット自動検出、カスタムタグ、アンカー）
- CDK 探索: 7 テスト（単一/複数テンプレート、リソース選択、エラーケース）
- NetworkMode: 10 テスト（パース、デフォルト、effective、as_str、CFN 統合）
- ネットワーク動作: 7 テスト（host メタデータ URI、extra_hosts、none メタデータスキップ）
- bollard 設定: 3 テスト（host/none/bridge モード別コンフィグ生成）
- バリデーション: 4 テスト（awsvpc 警告、host ポート不一致）
- CLI: 4 テスト（--from-cdk 引数パース）

合計 637 テスト通過。
