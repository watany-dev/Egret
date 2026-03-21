# Egret 要件定義書

## 概要

ECS タスク定義をローカルで実行し、ECS アプリが期待する実行時契約（メタデータエンドポイント、クレデンシャルプロバイダ、dependsOn、ヘルスチェック）を満たすCLI ツール。

---

## 機能要件

### FR-1: タスク定義パース

| ID | 要件 | 状態 |
|----|------|------|
| FR-1.1 | ECS タスク定義 JSON をパースできる | ✅ 実装済み |
| FR-1.2 | `family`, `containerDefinitions` の主要フィールドに対応する | ✅ 実装済み |
| FR-1.3 | `name`, `image`, `command`, `entryPoint`, `environment`, `portMappings` に対応する | ✅ 実装済み |
| FR-1.4 | `cpu`, `memory`, `memoryReservation` に対応する | ✅ 実装済み |
| FR-1.5 | `secrets` (`name`, `valueFrom`) に対応する | ✅ 実装済み |
| FR-1.6 | 未知フィールドを無視する（`deny_unknown_fields` を使わない） | ✅ 実装済み |
| FR-1.7 | バリデーション: `family` 空文字、`containerDefinitions` 空配列、`name`/`image` 空文字を検出する | ✅ 実装済み |

### FR-2: コンテナ実行

| ID | 要件 | 状態 |
|----|------|------|
| FR-2.1 | `egret run -f <file>` でタスク定義からコンテナを起動できる | ✅ 実装済み |
| FR-2.2 | 専用 bridge ネットワーク (`egret-<family>`) を作成する | ✅ 実装済み |
| FR-2.3 | コンテナ名で DNS 解決できる（ネットワークエイリアス） | ✅ 実装済み |
| FR-2.4 | 複数コンテナの順次起動に対応する | ✅ 実装済み |
| FR-2.5 | コンテナのログをプレフィックス付きでストリーム表示する | ✅ 実装済み |
| FR-2.6 | `Ctrl+C` でグレースフルシャットダウンする | ✅ 実装済み |

### FR-3: コンテナ停止

| ID | 要件 | 状態 |
|----|------|------|
| FR-3.1 | `egret stop <family>` で特定タスクのコンテナを停止・削除できる | ✅ 実装済み |
| FR-3.2 | `egret stop --all` で全 Egret 管理コンテナを停止・削除できる | ✅ 実装済み |
| FR-3.3 | 停止時にネットワークも削除する | ✅ 実装済み |
| FR-3.4 | 停止・削除エラーはベストエフォートで処理する（次のリソースに進む） | ✅ 実装済み |

### FR-4: ローカルオーバーライド

| ID | 要件 | 状態 |
|----|------|------|
| FR-4.1 | `--override` フラグでオーバーライドファイルを指定できる | ✅ 実装済み |
| FR-4.2 | コンテナイメージの上書きに対応する | ✅ 実装済み |
| FR-4.3 | 環境変数の追加・上書きに対応する（キーベースのマージ） | ✅ 実装済み |
| FR-4.4 | ポートマッピングの全置換に対応する | ✅ 実装済み |
| FR-4.5 | 未知コンテナ名は警告を出してスキップする | ✅ 実装済み |

### FR-5: Secrets 解決

| ID | 要件 | 状態 |
|----|------|------|
| FR-5.1 | `--secrets` フラグでシークレットマッピングファイルを指定できる | ✅ 実装済み |
| FR-5.2 | `valueFrom` の ARN をローカル値にマッピングできる | ✅ 実装済み |
| FR-5.3 | 解決した値を環境変数として注入する | ✅ 実装済み |
| FR-5.4 | ARN がマッピングにない場合はエラーにする（fail-fast） | ✅ 実装済み |
| FR-5.5 | `--secrets` 未指定だが `secrets` フィールドがある場合は警告を出す | ✅ 実装済み |

### FR-6: コンテナランタイム互換性

| ID | 要件 | 状態 |
|----|------|------|
| FR-6.1 | Docker をサポートする | ✅ 実装済み |
| FR-6.2 | Podman をサポートする（Docker 互換 API 経由） | ✅ 実装済み |
| FR-6.3 | `--host` フラグまたは `CONTAINER_HOST` 環境変数でソケットを指定できる | ✅ 実装済み |
| FR-6.4 | Podman ソケットを自動検出する（rootless → rootful の順） | ✅ 実装済み |

### FR-7: メタデータ + クレデンシャルサイドカー

| ID | 要件 | 状態 |
|----|------|------|
| FR-7.1 | `ECS_CONTAINER_METADATA_URI_V4` エンドポイントをモックする | ✅ 実装済み |
| FR-7.2 | タスクメタデータ JSON を返す | ✅ 実装済み |
| FR-7.3 | クレデンシャルプロバイダ（`/credentials`）をモックする | ✅ 実装済み |
| FR-7.4 | 各アプリコンテナに環境変数を自動注入する | ✅ 実装済み |
| FR-7.5 | `taskRoleArn` / `executionRoleArn` をパースする | ✅ 実装済み |
| FR-7.6 | `--no-metadata` フラグでサイドカーを無効化できる | ✅ 実装済み |
| FR-7.7 | `host.docker.internal` で全プラットフォームからサーバーに到達できる | ✅ 実装済み |
| FR-7.8 | AWS クレデンシャル取得失敗時は警告のみで続行する | ✅ 実装済み |

### FR-8: dependsOn + ヘルスチェック

| ID | 要件 | 状態 |
|----|------|------|
| FR-8.1 | `dependsOn` の DAG 解決（トポロジカルソート）に対応する | 🔲 未実装 |
| FR-8.2 | 起動条件（`START`, `COMPLETE`, `SUCCESS`, `HEALTHY`）に対応する | 🔲 未実装 |
| FR-8.3 | 循環依存を検出してエラーにする | 🔲 未実装 |
| FR-8.4 | `healthCheck` を Docker HEALTHCHECK として設定する | 🔲 未実装 |
| FR-8.5 | essential コンテナ停止時にタスク全体を停止する | 🔲 未実装 |

### FR-9: UX 改善

| ID | 要件 | 状態 |
|----|------|------|
| FR-9.1 | Bind mount ベースの volume に対応する | 🔲 未実装 |
| FR-9.2 | ログを色分けマルチプレクスする | 🔲 未実装 |
| FR-9.3 | `egret ps` で実行中タスク一覧を表示する | 🔲 未実装 |
| FR-9.4 | `egret logs <container>` で特定コンテナのログを表示する | 🔲 未実装 |

### FR-10: バリデーション + プロジェクト初期化

| ID | 要件 | 状態 |
|----|------|------|
| FR-10.1 | `egret validate` でタスク定義を静的解析できる（イメージ形式、ポート競合、ARN形式等） | 🔲 未実装 |
| FR-10.2 | `egret init` でスターターファイル（タスク定義、override、secrets テンプレート）を生成できる | 🔲 未実装 |
| FR-10.3 | `--dry-run` で起動せずにコンテナ構成を確認できる（secrets 値は伏字） | 🔲 未実装 |
| FR-10.4 | バリデーションエラーにフィールドパス・期待型・修正提案を含める | 🔲 未実装 |

### FR-11: 可観測性 + 診断

| ID | 要件 | 状態 |
|----|------|------|
| FR-11.1 | `egret ps` でリソース使用量・ヘルスチェック状態・依存関係を表示できる | 🔲 未実装 |
| FR-11.2 | `egret ps` の出力形式を `--output json/wide` で切り替えできる | 🔲 未実装 |
| FR-11.3 | `egret inspect` で実行中タスクの詳細（実効設定、ネットワーク構成）を表示できる | 🔲 未実装 |
| FR-11.4 | `egret stats` でライブリソース使用量（CPU、メモリ、I/O）を表示できる | 🔲 未実装 |
| FR-11.5 | `egret history` で実行履歴を記録・表示できる | 🔲 未実装 |
| FR-11.6 | `--events` でライフサイクルイベントを NDJSON 形式で出力できる | 🔲 未実装 |

### FR-12: ワークフロー高速化

| ID | 要件 | 状態 |
|----|------|------|
| FR-12.1 | `egret watch` でファイル変更時にタスクを自動再起動できる | 🔲 未実装 |
| FR-12.2 | `egret diff` でタスク定義をセマンティックに比較できる | 🔲 未実装 |
| FR-12.3 | `--profile` で設定プロファイル（override + secrets）を切り替えできる | 🔲 未実装 |
| FR-12.4 | `egret compose-import` で docker-compose.yml を ECS タスク定義に変換できる | 🔲 未実装 |
| FR-12.5 | `egret completions` でシェル補完スクリプト（bash/zsh/fish）を生成できる | 🔲 未実装 |

---

## 非機能要件

| ID | 要件 | 状態 |
|----|------|------|
| NFR-1 | `unsafe` コード禁止 | ✅ 適用中 |
| NFR-2 | clippy pedantic/nursery/cargo 準拠 | ✅ 適用中 |
| NFR-3 | `unwrap` 使用禁止（deny） | ✅ 適用中 |
| NFR-4 | テストカバレッジ 95% 以上（cargo-tarpaulin） | ✅ 適用中 |
| NFR-5 | cargo-deny による脆弱性・ライセンス監査 | ✅ 適用中 |
| NFR-6 | Rust edition 2024、MSRV 1.93.0 | ✅ 適用中 |

---

## 対象外（明示的に除外）

- Fargate 完全再現 / ENI 完全互換
- ALB / Cloud Map / Service Connect
- Capacity providers / Deployment circuit breaker
- FireLens 本番同等挙動
- Service / Auto Scaling / ローリングデプロイ
- コンテナイメージのビルド（Docker/Buildah/Kaniko の責務）
- Prometheus / Grafana 等の外部監視スタック連携
- awsvpc ネットワークモード完全再現
- Service Mesh / Service Connect
