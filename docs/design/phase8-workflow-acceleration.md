# Phase 8: ワークフロー高速化 設計書

## 概要

Phase 8 は **edit-run-debug サイクルの短縮**を目標とする。本フェーズでは以下の4機能を実装する。

- **`egret completions <shell>`** — bash/zsh/fish のシェル補完スクリプト生成
- **`egret diff <file1> <file2>`** — タスク定義のセマンティック差分表示（カラー出力対応）
- **`--profile`** — 設定プロファイルによる override/secrets の規約ベース自動ロード
- **`egret watch`** — ファイル変更監視 + 自動再起動

対応要件: FR-12.1, FR-12.2, FR-12.3, FR-12.4

---

## モジュール配置

| ファイル | 責務 |
|---------|------|
| `src/cli/completions.rs` | `egret completions` コマンド（補完スクリプト生成） |
| `src/cli/diff.rs` | `egret diff` コマンド（セマンティック差分ロジック + カラー出力） |
| `src/cli/watch.rs` | `egret watch` コマンド（ファイル変更監視 + 自動再起動） |
| `src/cli/mod.rs` | `CompletionsArgs`, `DiffArgs`, `WatchArgs` 型定義、`Command` enum 拡張 |
| `src/main.rs` | `Completions`, `Diff`, `Watch` ディスパッチ追加 |
| `src/profile/mod.rs` | プロファイル設定（別設計書: `phase8-profile.md`） |

---

## 型定義

### CLI 層（`src/cli/mod.rs`）

```rust
#[derive(Parser)]
pub struct CompletionsArgs {
    /// Shell type (bash, zsh, fish)
    pub shell: clap_complete::Shell,
}

#[derive(Parser)]
pub struct DiffArgs {
    /// First task definition file
    pub file1: PathBuf,
    /// Second task definition file
    pub file2: PathBuf,
}
```

---

## 公開 API

### `src/cli/completions.rs`

| 関数 | 説明 |
|------|------|
| `execute(args)` | stdout に補完スクリプトを出力（`#[cfg(not(tarpaulin_include))]`） |
| `generate_to_writer(shell, writer)` | 任意の `Write` 実装に補完スクリプトを生成（テスト可能） |

### `src/cli/diff.rs`

| 関数 | 説明 |
|------|------|
| `execute(args)` | ファイル読み込み + stdout 出力（`#[cfg(not(tarpaulin_include))]`） |
| `diff_from_json(json1, json2)` | JSON 文字列から差分文字列を生成（テスト用、`#[cfg(test)]`） |
| `diff_task_definitions(td1, td2)` | コア差分ロジック |

---

## データフロー

### `egret completions <shell>`

```
CLI arg (Shell enum)
    │
    ▼
clap_complete::generate(shell, &mut cmd, "egret", stdout)
    │
    ▼
stdout (shell completion script)
```

### `egret diff <file1> <file2>`

```
file1, file2
    │
    ▼
TaskDefinition::from_file() × 2
    │
    ▼
diff_task_definitions(td1, td2)
    │  family 比較
    │  コンテナ単位: name をキーに BTreeMap 化
    │    → 追加/削除/変更を検出
    │  各コンテナ内:
    │    image, essential, command, entryPoint
    │    environment (BTreeMap<name, value> で差分)
    │    portMappings (container_port をキーに差分)
    │    dependsOn (container_name をキーに差分)
    │    healthCheck (各フィールド個別比較)
    │    mountPoints (source_volume をキーに差分)
    │    cpu, memory, memoryReservation
    ▼
stdout (formatted diff output)
```

### diff 出力形式

```
family: my-app → my-app-v2

=== Container: app ===
  image: nginx:1.24 → nginx:1.25
  + environment: NEW_VAR=value
  - environment: OLD_VAR=old_value
  ~ environment: SHARED: v1 → v2

=== Container: redis (added) ===
  image: redis:7

=== Container: old-sidecar (removed) ===
```

差分なし: `No differences found.`

---

## エラーハンドリング

| ケース | 挙動 | 型 |
|--------|------|---|
| diff: ファイル読み込み失敗 | `TaskDefError::ReadFile` を返す | `TaskDefError` |
| diff: JSON パース失敗 | `TaskDefError::ParseJson` を返す | `TaskDefError` |
| diff: バリデーション失敗 | `TaskDefError::Validation` を返す | `TaskDefError` |
| completions: 無効なシェル名 | clap がパースエラーを返す | clap エラー |

---

## 技術選定

| 項目 | 選定 | 理由 |
|------|------|------|
| 補完生成 | `clap_complete` v4 | clap 公式の補完生成クレート。clap 4 と同一バージョン体系 |
| diff 比較方法 | 手動フィールド比較 | `TaskDefinition` は `PartialEq` 未実装。derive 追加は影響範囲が大きいため見送り |
| diff 出力 | プレーンテキスト | 色付けは将来拡張。新クレート依存を最小化 |
| コレクション比較 | `BTreeMap` | キーの自然順序でソートされた出力を保証 |

---

## テスト

| テスト対象 | テスト数 |
|-----------|---------|
| `cli/completions.rs` — bash/zsh/fish 生成 | 3 |
| `cli/mod.rs` — completions/diff CLI パース | 4 |
| `cli/diff.rs` — セマンティック差分 | 13 |
| **合計** | **20** |

---

## 追加実装

### `egret watch` (FR-12.1)

| 関数 | 説明 |
|------|------|
| `execute(args, host)` | ファイル監視ループ + コンテナ再起動（`#[cfg(not(tarpaulin_include))]`） |
| `collect_watch_paths(args)` | 監視対象パスの一覧を生成（テスト可能） |
| `validate_watch_paths(paths)` | パスの存在チェック（テスト可能） |
| `load_and_run_task(...)` | タスク定義読み込み + コンテナ起動（`#[cfg(not(tarpaulin_include))]`） |

技術選定: `notify` v7（MIT/Apache-2.0）、`tokio::sync::mpsc` でブリッジ。

### カラー diff 出力

| 関数 | 説明 |
|------|------|
| `colorize_diff(plain)` | プレーンテキスト差分にANSIカラーを適用 |

`DiffArgs` に `--no-color` フラグ追加。raw ANSI コード使用（新規依存なし）。

### 設定プロファイル (FR-12.3)

別設計書参照: `phase8-profile.md`
