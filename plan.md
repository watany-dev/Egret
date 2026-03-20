# review-codecommit → Egret 移植プラン

## 概要

review-codecommit（TypeScript/Bun/Ink）のCLAUDE.mdとskills（update-design, update-docs）をEgret（Rust/Cargo）向けに適応・移植する。

---

## 1. CLAUDE.md の拡張

現在のEgretのCLAUDE.mdは基本情報（Overview, Build, Architecture, Dependencies）のみ。以下のセクションを追加する。

### 追加セクション

#### 1-1. Completion Requirements（コミット前の必須チェック）
- review-codecommitの`bun run ci`に相当する`make check`をコミット前に必須とするルールを追加
- `make check`が実行する内容を明記（fmt-check → lint → test）

#### 1-2. プロジェクト基本方針
review-codecommitの方針をEgret向けに書き換え：
- **目的**: ECSタスク定義をローカルで実行し、ECSアプリが期待する実行時契約を満たす
- **技術方針**: 最小依存、Rust/Cargoエコシステム活用、安全性重視（`unsafe`禁止）
- **テスト品質**: Docker APIはモック、統合テストでE2E確認

#### 1-3. TDDサイクル
- そのまま移植（言語非依存の開発手法）

#### 1-4. Tidy First? (Kent Beck)
- そのまま移植（言語非依存の設計原則）
- パターン例をRustに適したものに微調整（例: Extract Helper → 関数抽出、Guard Clauses → early return/`?`演算子）

#### 1-5. イテレーション単位
- そのまま移植（言語非依存の開発方針）

### 変更しない部分
- Project Overview, Build & Development, Architecture, Key Dependencies は現状維持

---

## 2. `.claude/settings.json` の作成

review-codecommitのsettings.jsonは空（`{}`）だったため、同じく空のファイルを作成し、構造を用意しておく。

---

## 3. Skills の移植

### 3-1. `.claude/skills/update-design/SKILL.md`

review-codecommitの設計書評価・改善スキルをEgret向けに適応：

**変更点**:
- frontmatterのdescriptionをEgretに合わせる
- Phase 1: `docs/design/*.md`の参照先はそのまま維持（Egretでもdocs/配下に設計書を置く想定）
- Phase 2の評価カテゴリ: InkコンポーネントをRustモジュール/構造体に置き換え
  - 「コンポーネント設計」→「モジュール・構造体設計」
  - 「Inkコンポーネントの Props 型定義」→「Rust構造体のフィールド定義、トレイト実装」
  - 「AWS SDK連携設計」→「Docker API (bollard) / ECS互換設計」
  - 「技術選定の根拠」→ clap, bollard, tokio, axum 等の選定理由
- Phase 3-4: ソースコードとの整合性チェックをRustの型・関数シグネチャに変更
- 記述ルール: 「TypeScriptで記述」→「Rustで記述」

### 3-2. `.claude/skills/update-docs/SKILL.md`

review-codecommitのドキュメント一括最新化スキルをEgret向けに適応：

**変更点**:
- Phase 2の対象ドキュメント:
  - 設計書: `docs/design/<feature>.md`（同じ構成）
  - 要件定義書: `docs/requirements.md`（まだ存在しないが、将来用に設定）
  - ロードマップ: `docs/ROADMAP.md`（Egret固有、更新対象に追加）
- Phase 3: README.md更新
  - `src/cli.tsx`の参照 → `src/cli/`に変更
  - `src/index.ts`の参照 → `src/main.rs`に変更
  - インポート文の検証 → Cargo.toml / `use`文の検証に変更
- Phase 4: 一貫性チェック
  - TypeScript固有の参照をRustに変更
  - `bun`/`npm`コマンド → `cargo`/`make`コマンド
- 記述ルール:
  - コード例は「Rust」で記述
  - README.mdは英語（維持）
  - 開発ドキュメントは日本語（維持）

---

## 4. ファイル構成（最終形）

```
Egret/
├── CLAUDE.md                              # 拡張済み
├── .claude/
│   ├── settings.json                      # 新規（空）
│   └── skills/
│       ├── update-design/
│       │   └── SKILL.md                   # 新規（Egret適応版）
│       └── update-docs/
│           └── SKILL.md                   # 新規（Egret適応版）
└── ... (既存ファイルは変更なし)
```

---

## 5. 作業手順

1. ブランチ `claude/plan-claude-migration-71yVw` を作成
2. CLAUDE.mdに新セクションを追加
3. `.claude/settings.json` を作成
4. `.claude/skills/update-design/SKILL.md` を作成
5. `.claude/skills/update-docs/SKILL.md` を作成
6. コミット＆プッシュ
