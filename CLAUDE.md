# CLAUDE.md

## Project Overview

Egret is a local ECS task runner — run ECS task definitions locally by satisfying the runtime contract ECS apps expect (metadata endpoints, credential providers, dependsOn, health checks), without recreating the ECS control plane.

## Build & Development

```bash
make build      # cargo build --release
make test       # cargo test
make lint       # cargo clippy -- -D warnings
make fmt        # cargo fmt
make fmt-check  # cargo fmt -- --check
make check      # fmt-check + lint + test
```

## Architecture

- `src/cli/` — CLI commands (clap): run, stop, version
- `src/taskdef/` — ECS task definition JSON parser and types
- `src/docker/` — Docker Engine API client (bollard)
- `src/orchestrator/` — Container lifecycle and dependsOn DAG
- `src/metadata/` — ECS metadata endpoint mock (axum)
- `src/credentials/` — Credential provider mock
- `src/secrets/` — Secrets local resolver

## Key Dependencies

- `clap` — CLI framework
- `bollard` — Docker API client
- `tokio` — Async runtime
- `serde`/`serde_json` — JSON handling
- `axum` — HTTP server for metadata/credentials
- `tracing` — Structured logging
- `anyhow`/`thiserror` — Error handling

## Completion Requirements

Before committing, **must** run `make check` to execute all checks that CI will run:
```bash
make check
```

This runs the following in order (matching the GitHub Actions pipeline exactly):
1. `fmt-check` — rustfmt formatting check
2. `lint` — clippy with `-D warnings`
3. `test` — cargo test

**Do not skip any of these steps.** CI failures on push are caused by missing checks locally.

## プロジェクト基本方針

### 目的
ECSタスク定義をローカルで実行し、ECSアプリが期待する実行時契約（メタデータエンドポイント、クレデンシャルプロバイダ、dependsOn、ヘルスチェック）を満たす。

### 技術方針
- **最小依存**: 必要十分なクレートのみに依存し、軽量で高速な実装を維持
- **安全性重視**: `unsafe` コード禁止、clippy pedantic 準拠、`unwrap` 使用警告
- **Docker API活用**: bollard クレートを通じた Docker Engine API との連携
- **テスト品質**: Docker API はモック、統合テストで E2E 確認

## TDDサイクル
各機能は以下のサイクルで実装します:
1. **Red**: テストを書く（失敗する）
2. **Green**: 最小限の実装でテストを通す
3. **Refactor**: コードを改善する

## Tidy First? (Kent Beck)
機能変更の前に、まずコードを整理（tidy）するかを検討します:

**原則**:
- **構造的変更と機能的変更を分離する**: tidyingは別コミットで行う
- **小さく整理してから変更する**: 大きなリファクタリングより、小さな整理を積み重ねる
- **読みやすさを優先**: 次の開発者（未来の自分を含む）のために整理する

**Tidying パターン**:
1. **Guard Clauses**: ネストを減らすために早期リターン・`?` 演算子を使う
2. **Dead Code**: 使われていないコードを削除
3. **Normalize Symmetries**: 似た処理は同じ形式で書く
4. **Extract Helper**: 再利用可能な部分を関数・トレイトに抽出
5. **One Pile**: 散らばった関連コードを一箇所にまとめる
6. **Explaining Comments**: 理解しにくい箇所にコメントを追加
7. **Explaining Variables**: 複雑な式を説明的な変数に分解

**タイミング**:
- 変更対象のコードが読みにくい → Tidy First
- 変更が簡単にできる状態 → そのまま実装
- Tidyingのコストが高すぎる → 機能変更後に検討

## イテレーション単位
機能を最小単位に分割し、各イテレーションで1つの機能を完成させます。各イテレーションでコミットを行います。
