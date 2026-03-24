# Phase 8: ワークフロー高速化 設計書

## 概要

Phase 8 は **edit-run-debug サイクルの短縮**を目標とする。本フェーズでは以下の3機能を実装する。

- **`lecs completions <shell>`** — bash/zsh/fish のシェル補完スクリプト生成
- **`--profile`** — 設定プロファイルによる override/secrets の規約ベース自動ロード
- **`lecs watch`** — ファイル変更監視 + 自動再起動

対応要件: FR-12.1, FR-12.3, FR-12.4

---

## モジュール配置

| ファイル | 責務 |
|---------|------|
| `src/cli/completions.rs` | `lecs completions` コマンド（補完スクリプト生成） |
| `src/cli/watch.rs` | `lecs watch` コマンド（ファイル変更監視 + 自動再起動） |
| `src/cli/mod.rs` | `CompletionsArgs`, `WatchArgs` 型定義、`Command` enum 拡張 |
| `src/main.rs` | `Completions`, `Watch` ディスパッチ追加 |
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
```

---

## 公開 API

### `src/cli/completions.rs`

| 関数 | 説明 |
|------|------|
| `execute(args)` | stdout に補完スクリプトを出力（`#[cfg(not(tarpaulin_include))]`） |
| `generate_to_writer(shell, writer)` | 任意の `Write` 実装に補完スクリプトを生成（テスト可能） |

---

## データフロー

### `lecs completions <shell>`

```
CLI arg (Shell enum)
    │
    ▼
clap_complete::generate(shell, &mut cmd, "lecs", stdout)
    │
    ▼
stdout (shell completion script)
```

---

## エラーハンドリング

| ケース | 挙動 | 型 |
|--------|------|---|
| completions: 無効なシェル名 | clap がパースエラーを返す | clap エラー |

---

## 技術選定

| 項目 | 選定 | 理由 |
|------|------|------|
| 補完生成 | `clap_complete` v4 | clap 公式の補完生成クレート。clap 4 と同一バージョン体系 |

---

## テスト

| テスト対象 | テスト数 |
|-----------|---------|
| `cli/completions.rs` — bash/zsh/fish 生成 | 3 |
| `cli/mod.rs` — completions CLI パース | 2 |
| **合計** | **5** |

---

## 追加実装

### `lecs watch` (FR-12.1)

| 関数 | 説明 |
|------|------|
| `execute(args, host)` | ファイル監視ループ + コンテナ再起動（`#[cfg(not(tarpaulin_include))]`） |
| `collect_watch_paths(args)` | 監視対象パスの一覧を生成（テスト可能） |
| `validate_watch_paths(paths)` | パスの存在チェック（テスト可能） |
| `load_and_run_task(...)` | タスク定義読み込み + コンテナ起動（`#[cfg(not(tarpaulin_include))]`） |

技術選定: `notify` v8（MIT/Apache-2.0）、`tokio::sync::mpsc` でブリッジ。

### 設定プロファイル (FR-12.3)

別設計書参照: `phase8-profile.md`
