# Phase 3: Metadata + Credentials Sidecar — 設計書

## Context

ECS 上で動くアプリの多くは `ECS_CONTAINER_METADATA_URI_V4` でコンテナ/タスクのメタデータを、`AWS_CONTAINER_CREDENTIALS_FULL_URI` で AWS クレデンシャルを取得する。Phase 3 ではこれらのエンドポイントをローカルでモックし、コンテナに環境変数として自動注入することで、本番 ECS アプリをそのままローカルで実行可能にする。

---

## アーキテクチャ

```
                     ┌─────────────────────────────────┐
                     │    Egret Host Process            │
                     │                                  │
                     │  ┌───────────────────────────┐   │
                     │  │  axum HTTP Server          │   │
                     │  │  0.0.0.0:<random-port>     │   │
                     │  │                            │   │
                     │  │  GET /v4/{name}            │───── Container metadata
                     │  │  GET /v4/{name}/task       │───── Task metadata
                     │  │  GET /v4/{name}/stats      │───── 501 (future)
                     │  │  GET /v4/{name}/task/stats │───── 501 (future)
                     │  │  GET /credentials          │───── AWS credentials
                     │  │  GET /health               │───── Health check
                     │  └───────────────────────────┘   │
                     └─────────────────────────────────┘
                                    ▲
                                    │ http://host.docker.internal:<port>
                     ┌──────────────┼──────────────────┐
                     │  egret-<family> network          │
                     │              │                   │
                     │  ┌─────┐  ┌─────┐  ┌─────┐     │
                     │  │ app │  │ web │  │redis│     │
                     │  └─────┘  └─────┘  └─────┘     │
                     └──────────────────────────────────┘
```

### 環境変数注入

コンテナに注入される環境変数:
- `ECS_CONTAINER_METADATA_URI_V4=http://host.docker.internal:<port>/v4/<container-name>`
- `AWS_CONTAINER_CREDENTIALS_FULL_URI=http://host.docker.internal:<port>/credentials`

`FULL_URI` を使う理由: `RELATIVE_URI` は `169.254.170.2` をベースとする前提。`FULL_URI` は任意のホスト:ポートを指定でき、AWS SDK が `RELATIVE_URI` 未設定時に `FULL_URI` をフォールバック参照する。

### host.docker.internal 対応

全プラットフォームで `extra_hosts: ["host.docker.internal:host-gateway"]` を HostConfig に設定。Docker Desktop (Mac/Win) では無害、Linux Docker / Podman では必要。

---

## 変更対象ファイル

| ファイル | 変更内容 |
|---------|---------|
| `Cargo.toml` | `axum`, `aws-config`, `aws-credential-types`, `chrono`, `reqwest` (dev) 追加 |
| `src/taskdef/mod.rs` | `task_role_arn`, `execution_role_arn` フィールド追加 |
| `src/container/mod.rs` | `extra_hosts` フィールド追加、`build_bollard_config` で HostConfig に反映 |
| `src/credentials/mod.rs` | `CredentialError`, `AwsCredentials`, `load_local_credentials()` 実装 |
| `src/metadata/mod.rs` | メタデータ型、サーバー、ルートハンドラ全て実装 |
| `src/cli/mod.rs` | `--no-metadata` フラグ追加 |
| `src/cli/run.rs` | サーバー起動 + 環境変数注入の統合 |

---

## 主要な型

### `AwsCredentials` (credentials/mod.rs)

ECS クレデンシャルプロバイダ互換の JSON 形式:
```json
{
  "AccessKeyId": "AKIA...",
  "SecretAccessKey": "...",
  "Token": "...",
  "Expiration": "2026-03-21T01:00:00Z",
  "RoleArn": "arn:aws:iam::123:role/my-role"
}
```

### `TaskMetadata` / `ContainerMetadata` (metadata/mod.rs)

ECS v4 メタデータ互換の PascalCase JSON。カスタム rename:
- `TaskARN`, `ContainerARN`, `ImageID`, `DockerId`, `IPv4Addresses`, `CPU`

### `ServerState` (metadata/mod.rs)

`Arc<RwLock<ServerState>>` で包み、コンテナ作成後に Docker ID を更新可能。

---

## データフロー

1. `TaskDefinition::from_file()` + Override + Secrets（既存）
2. `--no-metadata` でなければ:
   a. `load_local_credentials()` — 失敗時は warn + None
   b. `ServerState` 構築（`build_task_metadata`, `build_container_metadata`）
   c. `MetadataServer::start()` — ランダムポートで起動
3. `build_container_config()` で `metadata_port` に応じて環境変数注入 + `extra_hosts` 設定
4. コンテナ作成・起動
5. `update_container_id()` で Docker ID を反映
6. ログストリーム + Ctrl+C 待機
7. `MetadataServer::shutdown()` + 既存クリーンアップ

---

## エラーハンドリング

| ケース | 挙動 |
|--------|------|
| AWS クレデンシャル取得失敗 | warning + メタデータのみ提供 |
| メタデータサーバーバインド失敗 | hard error |
| 不明なコンテナ名へのリクエスト | 404 |
| stats エンドポイント | 501 Not Implemented |
| `--no-metadata` 指定時 | サーバー起動しない、環境変数も注入しない |

---

## スコープ外

- `/v4/{id}/stats` の bollard プロキシ実装（501 を返す）
- クレデンシャルの自動リフレッシュ（起動時に1回取得）
- `AWS_CONTAINER_AUTHORIZATION_TOKEN` ヘッダー検証
- `169.254.170.2` でのリッスン（`FULL_URI` で代替）

---

## テスト

97 テスト:
- `taskdef`: `parse_task_role_arn`, `parse_no_role_arns_default`
- `container`: `build_bollard_config_extra_hosts`, `build_bollard_config_empty_extra_hosts`
- `credentials`: シリアライゼーション検証（4テスト）
- `metadata`: 型検証（8テスト）+ サーバー統合テスト（10テスト）
- `cli/run`: `build_container_config_with/without_metadata_port`, `parse_run_with/without_no_metadata_flag`
