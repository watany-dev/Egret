# Phase 8: 設定プロファイル (`--profile`) 設計書

## 概要

Phase 8 の設定プロファイル機能は、環境ごと（dev/staging/prod）に異なる override ファイルや secrets ファイルを規約ベースで自動ロードする。`--profile dev` で `egret-override.dev.json` / `secrets.dev.json` を自動解決し、`.egret.toml` でデフォルトプロファイルを設定可能にする。

対応要件: FR-12.3

---

## ファイルフォーマット設計

### `.egret.toml`

```toml
# デフォルトプロファイル名
default_profile = "dev"
```

最小スキーマ。`default_profile` のみを設定可能。

**設計根拠**:
- プロファイルはファイル名規約で解決するため、ファイルパスの明示的設定は不要
- `-f` フラグ（タスク定義パス）は常に必須であり、`.egret.toml` での上書きはスコープ外
- 未知フィールドは無視する（前方互換性）

### 規約ベースのファイル名

| プロファイル名 | Override ファイル | Secrets ファイル |
|--------------|-----------------|----------------|
| `dev` | `egret-override.dev.json` | `secrets.dev.json` |
| `staging` | `egret-override.staging.json` | `secrets.staging.json` |
| `prod` | `egret-override.prod.json` | `secrets.prod.json` |

ファイルはタスク定義ファイルの親ディレクトリから解決する。

---

## Rust 型定義

### `src/profile/mod.rs`

```rust
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("failed to read config file {path}: {source}")]
    ReadConfig { path: PathBuf, source: std::io::Error },
    #[error("failed to parse config file {path}: {source}")]
    ParseConfig { path: PathBuf, source: toml::de::Error },
    #[error("invalid profile name '{name}': must match [A-Za-z0-9_-]+")]
    InvalidProfileName { name: String },
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct EgretConfig {
    pub default_profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPaths {
    pub override_path: Option<PathBuf>,
    pub secrets_path: Option<PathBuf>,
}
```

### 公開 API

```rust
impl EgretConfig {
    pub fn from_file(path: &Path) -> Result<Self, ProfileError>;
    pub fn from_toml(toml_str: &str, source_path: &Path) -> Result<Self, ProfileError>;
}

pub fn validate_profile_name(name: &str) -> Result<(), ProfileError>;
pub fn find_config(start_dir: &Path) -> Option<PathBuf>;
pub fn load_config_with_warning(base_dir: &Path) -> Option<EgretConfig>;
pub fn effective_profile<'a>(cli_profile: Option<&'a str>, config: Option<&'a EgretConfig>) -> Option<&'a str>;
pub fn profile_override_path(base_dir: &Path, profile: &str) -> PathBuf;
pub fn profile_secrets_path(base_dir: &Path, profile: &str) -> PathBuf;
pub fn resolve(
    base_dir: &Path,
    profile: Option<&str>,
    explicit_override: Option<&Path>,
    explicit_secrets: Option<&Path>,
) -> Result<ResolvedPaths, ProfileError>;
```

---

## データフロー

```
1. base_dir = args.task_definition.parent()
2. profile::load_config_with_warning(base_dir)
     → .egret.toml を上方探索、パース失敗時は tracing::warn! + None
3. profile::effective_profile(args.profile, config)
     → args.profile (CLI) > config.default_profile (.egret.toml) > None
4. profile::resolve(base_dir, effective_profile, args.override, args.secrets)
     → プロファイル名を validate_profile_name() で検証（不正文字 → エラー）
     各軸独立:
       - explicit flag あり → そのパスを使用
       - profile あり → 規約パスを生成 → ファイルが存在する場合のみ Some
       - 両方 None → None
5. resolved.override_path があれば OverrideConfig::from_file() でロード・適用
6. resolved.secrets_path があれば SecretsResolver::from_file() でロード・解決
7. 以降は既存フローと同一
```

### 優先順位

| 優先度 | ソース | 説明 |
|--------|--------|------|
| 1（最高） | `--override` / `--secrets` CLI フラグ | 明示的指定は常に最優先 |
| 2 | `--profile <name>` CLI フラグ | 規約ベースでファイル名を導出 |
| 3 | `.egret.toml` の `default_profile` | 暗黙のデフォルト |
| 4（最低） | なし | override/secrets なしで実行 |

---

## エラーハンドリング方針

| ケース | 挙動 | 理由 |
|--------|------|------|
| `.egret.toml` 読み込み/パース失敗 | `tracing::warn!` + 無視（`None` にフォールバック） | 設定ファイルの欠落はデフォルト動作 |
| プロファイル名に不正文字（`/`, `\`, `..`, スペース等） | hard error (`InvalidProfileName`) | パストラバーサル防止。`[A-Za-z0-9_-]+` のみ許可 |
| プロファイル規約ファイルが存在しない | サイレントスキップ（None） | 片方だけ使うケースも多い |
| 明示フラグのファイルが存在しない | hard error | ユーザが明示的に指定したため |

---

## CLI 変更

```
egret run -f <file> [--profile <name>] [--override <file>] [--secrets <file>]
egret validate -f <file> [--profile <name>] [--override <file>] [--secrets <file>]
```

`-p` は `--profile` の短縮形。

---

## テスト戦略

| テスト種別 | テスト数 | 対象 |
|-----------|---------|------|
| `EgretConfig` パース | 5 | `from_toml`, `from_file`, エラーケース |
| 規約パスビルダー | 4 | `profile_override_path`, `profile_secrets_path` |
| `find_config` 上方探索 | 4 | 現在dir, 親dir, 祖父dir, 未発見 |
| `resolve` 優先順位 | 8 | 全パターン（明示/プロファイル/なし × ファイル存在/非存在） |
| CLI パース | 5 | `--profile`, `-p`, 組み合わせ |
| `init` テンプレート | 2 | `.egret.toml` 生成 |
| **合計** | **28** | |

---

## 検証方法

```bash
make check    # fmt-check + lint + test + doc + deny
make coverage # 95% 以上を確認
```
