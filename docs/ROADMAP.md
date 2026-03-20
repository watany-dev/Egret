# Egret: Local ECS Task Runner — 実装計画

## Context

ECS にデプロイするアプリをローカルで動かすとき、ECS が提供する「実行時契約」（メタデータエンドポイント、クレデンシャルプロバイダ、dependsOn、ヘルスチェック等）がないと正しくテストできない。AWS 公式の `amazon-ecs-local-container-endpoints` は Metadata/Credentials のモックを提供するが、task definition ネイティブな CLI 体験・ネットワーク自動構築・dependsOn/ヘルスチェック統合はない。

Egret は「ECS control plane の再現」ではなく「ECS アプリが期待する実行時契約をローカルで満たす」ことに特化した CLI ツール。

---

## 開発エコシステム

### 主要依存クレート

| クレート | 用途 | Phase |
|---|---|---|
| `clap` (derive) | CLI フレームワーク | 0 |
| `tokio` | async ランタイム | 0 |
| `serde` / `serde_json` | JSON シリアライズ・デシリアライズ | 0 |
| `tracing` / `tracing-subscriber` | 構造化ログ | 0 |
| `anyhow` / `thiserror` | エラーハンドリング | 0 |
| `bollard` | Docker Engine API クライアント | 1 |
| `futures-util` | bollard の Stream 処理 | 1 |
| `axum` | メタデータ/クレデンシャル HTTP サーバー | 3（予定） |

### ディレクトリ構成

```
src/
├── main.rs              # エントリポイント (clap CLI)
├── cli/                 # CLI コマンド定義
├── taskdef/             # Task Definition JSON パーサ・型定義
├── docker/              # Docker Engine API クライアント
├── orchestrator/        # dependsOn DAG・ライフサイクル管理
├── metadata/            # ECS メタデータエンドポイントモック
├── credentials/         # クレデンシャルプロバイダモック
└── secrets/             # Secrets ローカル差し替え
```

---

## ロードマップ

### Phase 0: Skeleton ✅
**目標**: ビルド可能な CLI スケルトン + 開発エコシステム

- [x] `cargo init` + ディレクトリ作成 + 依存追加
- [x] clap による `egret run --task-definition <file>` スケルトン
- [x] `egret version` コマンド
- [x] Makefile, `rustfmt.toml`, CI workflow
- [x] `make check` が全て通ること

### Phase 1: Task Definition パース + 単一コンテナ実行 ✅
**目標**: `egret run -f task-def.json` で単一コンテナが Docker 上で動く

- [x] Task Definition JSON パーサ（serde で `containerDefinitions` の主要フィールド対応）
  - `name`, `image`, `command`, `entryPoint`, `environment`, `portMappings`
  - `cpu`, `memory`, `memoryReservation`, `essential`
- [x] bollard で Docker コンテナ作成・起動・ログストリーム表示
- [x] 専用 Docker network 作成（`egret-<task-name>`）
- [x] コンテナ名ベースの DNS 解決（Docker ネットワーク内）
- [x] `egret stop` でクリーンアップ（コンテナ停止 + ネットワーク削除）

### Phase 2: ローカルオーバーライド + Secrets 差し替え
**目標**: 本番 task definition をそのまま使いつつ、ローカル固有の設定を上書き

- [ ] オーバーライドファイル（`egret-override.json`）
  - 環境変数の追加・上書き
  - イメージタグの差し替え
  - ポートマッピング変更
- [ ] Secrets 解決
  - `valueFrom` の ARN → ローカルマッピングファイルから値を引く
  - `secrets.local.json`: `{ "arn:aws:secretsmanager:...": "local-value" }`

### Phase 3: Metadata + Credentials Sidecar
**目標**: `ECS_CONTAINER_METADATA_URI_V4` と `AWS_CONTAINER_CREDENTIALS_RELATIVE_URI` が動く

- [ ] axum ベースのメタデータ HTTP サーバー
  - `${ECS_CONTAINER_METADATA_URI_V4}` → コンテナメタデータ JSON
  - `${ECS_CONTAINER_METADATA_URI_V4}/task` → タスクメタデータ JSON
  - `${ECS_CONTAINER_METADATA_URI_V4}/stats` → Docker stats プロキシ
- [ ] クレデンシャルプロバイダ
  - `/creds` → ローカル AWS credentials を返す
  - `/role/{name}` → AssumeRole 結果を返す
  - `169.254.170.2` でリッスン
- [ ] 各アプリコンテナに環境変数を自動注入

### Phase 4: dependsOn + Health Check
**目標**: マルチコンテナ task の起動順序と健全性を制御

- [ ] `dependsOn` の DAG 解決（トポロジカルソート）
  - 条件: `START`, `COMPLETE`, `SUCCESS`, `HEALTHY`
  - 循環依存の検出・エラー
- [ ] Health Check 実行・監視
  - `healthCheck.command` を Docker HEALTHCHECK として設定
  - `interval`, `timeout`, `retries`, `startPeriod` 対応
- [ ] essential コンテナ停止時のタスク全体停止

### Phase 5: Volume + ログ + UX 改善
**目標**: 実用的な開発体験

- [ ] Bind mount ベースの volume（`volumes` + `mountPoints`）
- [ ] ログ統合（全コンテナのログを色分けマルチプレクス）
- [ ] `egret ps` — 実行中タスク一覧
- [ ] `egret logs <container>` — 特定コンテナのログ表示
- [x] Ctrl+C グレースフルシャットダウン（tokio signal handling）— Phase 1 で実装済み

---

## 対象外（明示的に除外）

- Fargate 完全再現 / ENI 完全互換
- ALB / Cloud Map / Service Connect
- Capacity providers / Deployment circuit breaker
- FireLens 本番同等挙動
- Service / Auto Scaling / ローリングデプロイ
