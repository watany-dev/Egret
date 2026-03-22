# Phase 6: バリデーション + Init + Dry-run 設計書

## 概要

Phase 6 はコンテナ起動**前**にエラーを検出し、プロジェクト開始を高速化する DX 機能を提供する。

- **`egret validate`** — タスク定義の静的解析（collect-all 方式の構造化診断）
- **`egret init`** — スターターファイル生成（テンプレートスキャフォールディング）
- **`--dry-run`** — 起動せずにコンテナ構成を確認（secrets 値は伏字）

対応要件: FR-10.1〜FR-10.4

---

## アーキテクチャ

```
egret validate -f task-def.json [--override ...] [--secrets ...]
    │
    ▼
TaskDefinition::from_json()
    │
    ▼
diagnostics::validate_extended(&task_def)
    │  check_image_format()
    │  check_port_conflicts()
    │  check_depends_on() (参照 + 循環)
    │  check_secret_arn_format()
    │  check_common_mistakes()
    │
    ▼ (--override 指定時)
diagnostics::validate_overrides(&task_def, &overrides)
    │
    ▼ (--secrets 指定時)
validate_secrets_coverage(&task_def, &resolver)
    │
    ▼
ValidationReport → stdout (エラー: exit 1 / 警告のみ: exit 0)

egret init [--dir .] [--image ...] [--family ...]
    │
    ▼
generate_task_definition() → task-definition.json
generate_override_template() → egret-override.json
generate_secrets_template() → secrets.local.json

egret run -f task-def.json --dry-run
    │
    ▼
parse → override → secrets resolve → display_dry_run() → exit 0
```

---

## モジュール配置

| ファイル | 責務 |
|---------|------|
| `src/taskdef/diagnostics.rs` | `Severity`, `ValidationDiagnostic`, `ValidationReport` 型定義 + 拡張バリデーション関数群 |
| `src/taskdef/mod.rs` | `pub mod diagnostics;` 宣言 |
| `src/cli/validate.rs` | `egret validate` コマンド実装 |
| `src/cli/init.rs` | `egret init` コマンド実装 |
| `src/cli/mod.rs` | `ValidateArgs`, `InitArgs` 型定義、`Command` enum 拡張、`RunArgs` に `--dry-run` 追加 |
| `src/cli/run.rs` | dry-run 分岐 + `display_dry_run()` + `format_container_dry_run()` |
| `src/main.rs` | `Validate`, `Init` ディスパッチ追加 |

---

## 型定義

### diagnostics 層（`src/taskdef/diagnostics.rs`）

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
pub struct ValidationDiagnostic {
    pub severity: Severity,
    pub field_path: String,       // e.g. "containerDefinitions[0].image"
    pub message: String,
    pub suggestion: Option<String>,
}

#[derive(Debug)]
pub struct ValidationReport {
    pub diagnostics: Vec<ValidationDiagnostic>,
}

impl ValidationReport {
    pub fn has_errors(&self) -> bool
    pub fn error_count(&self) -> usize
    pub fn warning_count(&self) -> usize
}

impl fmt::Display for ValidationDiagnostic  // "error: field - message (hint: suggestion)"
impl fmt::Display for ValidationReport       // 全診断 + "N error(s), M warning(s)"
```

**設計判断**: 既存の `TaskDefinition::validate()` (fail-fast, `TaskDefError::Validation`) はそのまま残す。`validate_extended()` は別途すべてのチェックを collect-all で実行し `ValidationReport` を返す。

### CLI 層（`src/cli/mod.rs`）

```rust
#[derive(Parser)]
pub struct ValidateArgs {
    #[arg(short = 'f', long = "task-definition")]
    pub task_definition: PathBuf,
    #[arg(short, long)]
    pub r#override: Option<PathBuf>,
    #[arg(short, long)]
    pub secrets: Option<PathBuf>,
}

#[derive(Parser)]
pub struct InitArgs {
    #[arg(short, long, default_value = ".")]
    pub dir: PathBuf,
    #[arg(long, default_value = "nginx:latest")]
    pub image: String,
    #[arg(long, default_value = "my-app")]
    pub family: String,
}

// RunArgs に追加:
#[arg(long)]
pub dry_run: bool,
```

---

## 公開 API

### `src/taskdef/diagnostics.rs`

| 関数 | シグネチャ | 説明 |
|------|----------|------|
| `validate_extended` | `fn(task_def: &TaskDefinition) -> ValidationReport` | 拡張バリデーション（collect-all） |
| `validate_overrides` | `fn(task_def: &TaskDefinition, overrides: &OverrideConfig) -> Vec<ValidationDiagnostic>` | オーバーライドのコンテナ名クロスバリデーション |

内部関数:
- `check_image_format(image, field_path)` — 空白、先頭/末尾 `/:` 、連続 `//` `::` 、非英数字開始を検出
- `check_port_conflicts(task_def)` — コンテナ内/跨ぎのホストポート競合を検出
- `check_depends_on(task_def)` — 参照存在チェック + 自己参照 + 循環依存検出（`orchestrator::resolve_start_order()` 再利用）
- `check_secret_arn_format(task_def)` — `arn:aws:secretsmanager:` プレフィックス + 6セグメント以上を検証
- `check_common_mistakes(task_def)` — 全コンテナ essential=false、ポートマッピング皆無を警告

### `src/cli/validate.rs`

| 関数 | 説明 |
|------|------|
| `execute(args: &ValidateArgs) -> Result<()>` | ファイル I/O ラッパー（`#[cfg(not(tarpaulin_include))]`） |
| `execute_from_json(task_json, override_json, secrets_json) -> Result<()>` | テスト可能なコアロジック |

### `src/cli/init.rs`

| 関数 | 説明 |
|------|------|
| `execute(args: &InitArgs) -> Result<()>` | テンプレートファイル生成 + 既存ファイルスキップ |
| `generate_task_definition(family, image) -> String` | `serde_json::json!` によるタスク定義テンプレート |
| `generate_override_template(container_name) -> String` | オーバーライドテンプレート |
| `generate_secrets_template() -> String` | シークレットマッピングテンプレート |

### `src/cli/run.rs`（追加）

| 関数 | 説明 |
|------|------|
| `display_dry_run(task_def, secret_names) -> String` | 全コンテナの解決済み構成をフォーマット |
| `format_container_dry_run(family, container, secret_names) -> String` | 単一コンテナの dry-run 表示（secrets は `******` でマスク） |

---

## データフロー

### `egret validate`

1. `TaskDefinition::from_json()` — パース失敗時は即時エラー
2. `diagnostics::validate_extended()` — 全チェックを collect-all で実行
3. `--override` 指定時: `OverrideConfig::from_json()` → `validate_overrides()` → 診断追加
4. `--secrets` 指定時: `SecretsResolver::from_json()` → `validate_secrets_coverage()` → 未カバー ARN を検出
5. `report.has_errors()` → true: exit 1 / false: exit 0

### `egret init`

1. 出力ディレクトリの存在確認
2. 各ファイル（`task-definition.json`, `egret-override.json`, `secrets.local.json`）の存在チェック
3. 存在しないファイルのみ生成、既存ファイルはスキップ + メッセージ表示
4. 次ステップのヒント表示（`egret validate`, `egret run`）

### `--dry-run`

1. タスク定義パース → オーバーライド適用 → Secrets 解決
2. `secret_names` を `HashSet<String>` として収集
3. `display_dry_run()` で各コンテナの構成を表示
4. `return Ok(())` — コンテナ起動なし

---

## エラーハンドリング

| ケース | 挙動 | 型 |
|--------|------|---|
| JSON パース失敗 | 即時 fail | `TaskDefError::ParseJson` |
| イメージ名形式不正 | 診断収集（Error） | `ValidationDiagnostic` |
| ホストポート競合 | 診断収集（Error） | `ValidationDiagnostic` |
| 循環依存 | 診断収集（Error） | `ValidationDiagnostic` |
| Secret ARN 形式不正 | 診断収集（Warning） | `ValidationDiagnostic` |
| 全コンテナ essential=false | 診断収集（Warning） | `ValidationDiagnostic` |
| ポートマッピング皆無 | 診断収集（Warning） | `ValidationDiagnostic` |
| Override の不明コンテナ名 | 診断収集（Error） | `ValidationDiagnostic` |
| Secret ARN 未カバー | 診断収集（Error） | `ValidationDiagnostic` |
| init でファイルが既存 | スキップ + 表示 | — |

---

## 技術選定

| 項目 | 選定 | 理由 |
|------|------|------|
| イメージ名バリデーション | 文字列操作 | 新規依存なし。`regex` クレート不要 |
| テンプレート生成 | `serde_json::json!` マクロ | 既存依存。型安全。pretty-print 対応 |
| 循環依存検出 | `orchestrator::resolve_start_order()` | Kahn's algorithm 実装済み。コード重複なし |
| 文字列組み立て | `std::fmt::Write` | clippy `format_push_string` 準拠 |

---

## 既存コード再利用

| 再利用対象 | ファイル | 用途 |
|-----------|---------|------|
| `TaskDefinition::from_json()` | `src/taskdef/mod.rs` | validate, init, dry-run |
| `OverrideConfig::from_json()` / `apply()` | `src/overrides/mod.rs` | validate, dry-run |
| `SecretsResolver::from_json()` / `resolve()` | `src/secrets/mod.rs` | validate, dry-run |
| `resolve_start_order()` | `src/orchestrator/mod.rs` | 循環依存検出 |
| `DependencyInfo` | `src/orchestrator/mod.rs` | `check_depends_on()` の入力構築 |

---

## テスト

| テスト対象 | テスト数 |
|-----------|---------|
| `ValidationDiagnostic` / `ValidationReport` 型 + Display | 5 |
| `check_image_format` | 5 |
| `check_port_conflicts` | 4 |
| `check_depends_on` | 5 |
| `check_secret_arn_format` | 4 |
| `check_common_mistakes` | 4 |
| `validate_overrides` | 3 |
| `cli/validate.rs` | 8 |
| `cli/init.rs` | 10 |
| `cli/run.rs` dry-run | 7 |
| `cli/mod.rs` CLI パース | 6 |
| **合計** | **~61** |
