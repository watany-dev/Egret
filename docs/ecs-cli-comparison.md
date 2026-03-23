# Lecs vs ECS CLI — 何が違うの？

## TL;DR

ECS CLI にも `ecs-cli local` というローカル実行機能がある。しかし、やり方が根本的に違う。

| 観点 | `ecs-cli local` | **Lecs** |
|------|------------------|----------|
| **仕組み** | タスク定義 → Docker Compose に変換 → `docker-compose up` | タスク定義 → **直接** Docker/Podman API で実行 |
| **メタデータ** | 別途 `amazon-ecs-local-container-endpoints` コンテナが必要 | **組み込み** (axum HTTP サーバー) |
| **dependsOn** | Docker Compose の `depends_on` に変換（条件は失われる） | **ECS の条件 (START/COMPLETE/SUCCESS/HEALTHY) をそのまま再現** |
| **現在の状態** | 非推奨 (deprecated) | 現行 |

---

## `ecs-cli local` の仕組み

```
ecs-cli local up --task-def-file task-definition.json
```

内部的にやっていること:

```
task-definition.json
       │
       ▼
  ecs-cli local create     ← タスク定義を Docker Compose に変換
       │
       ├── docker-compose.ecs-local.yml
       └── docker-compose.ecs-local.override.yml
       │
       ▼
  docker-compose up         ← Docker Compose で起動
       │
       ▼
  (オプション) amazon-ecs-local-container-endpoints コンテナ
       │                    ← メタデータ/クレデンシャルのモック
       └── 169.254.170.2 で待ち受け
```

### 変換時に失われるもの

ECS タスク定義から Docker Compose への変換で、ECS 固有の概念が**翻訳**される。この過程でいくつかの機能が失われる、または意味が変わる:

| ECS 機能 | `ecs-cli local` での扱い |
|-----------|--------------------------|
| `dependsOn` (START/COMPLETE/SUCCESS/HEALTHY) | Docker Compose の `depends_on` に変換。**COMPLETE/SUCCESS 条件は表現不可**。HEALTHY は `depends_on` + `condition: service_healthy` に変換されるが、Docker Compose バージョンに依存 |
| `secrets` (ARN 参照) | 変換されない。手動で Docker Compose の `environment` に書き直す必要がある |
| `essential` フラグ | Docker Compose に直接の対応概念がない |
| `taskRoleArn` / `executionRoleArn` | `amazon-ecs-local-container-endpoints` との組み合わせで一部対応 |

---

## Lecs の仕組み

```
lecs run -f task-definition.json
```

内部的にやっていること:

```
task-definition.json
       │
       ▼
  タスク定義パーサー        ← ECS JSON をネイティブに解析
       │
       ▼
  DAG 解決                 ← dependsOn のトポロジカルソート
       │
       ▼
  Docker/Podman API        ← bollard クレートで直接コンテナ操作
  (変換なし、直接実行)
       │
       ├── ブリッジネットワーク自動作成 + DNS エイリアス
       ├── 条件待ち (START/COMPLETE/SUCCESS/HEALTHY)
       ├── ヘルスチェックポーリング
       └── ログストリーミング (コンテナ名プレフィックス付き)
       │
       ▼
  組み込みメタデータサーバー (axum)
       │
       ├── ECS_CONTAINER_METADATA_URI_V4 → コンテナ/タスクメタデータ
       └── AWS_CONTAINER_CREDENTIALS_FULL_URI → AWS クレデンシャル
```

**中間形式への変換がない。** タスク定義を直接解釈して実行するため、ECS 固有の意味論（dependsOn 条件、essential フラグ、シークレット ARN など）がそのまま保持される。

---

## 詳細比較

### 入力と実行

| 観点 | `ecs-cli local` | **Lecs** |
|------|------------------|----------|
| 入力形式 | ECS タスク定義 JSON | ECS タスク定義 JSON |
| 実行方式 | Docker Compose 経由 | Docker/Podman API 直接 |
| 追加依存 | `docker-compose` が必要 | なし（単一バイナリ） |
| コンテナランタイム | Docker のみ | Docker **および** Podman |

### ECS 機能の再現度

| ECS 機能 | `ecs-cli local` | **Lecs** |
|----------|------------------|----------|
| コンテナ起動 | ✅ | ✅ |
| 環境変数 | ✅ | ✅ |
| ポートマッピング | ✅ | ✅ |
| ヘルスチェック | ✅ (Docker Compose HEALTHCHECK) | ✅ (ECS 形式をそのまま変換) |
| `dependsOn` START | ✅ (`depends_on`) | ✅ |
| `dependsOn` HEALTHY | △ (Compose v2.1+ 必要) | ✅ |
| `dependsOn` COMPLETE | ❌ | ✅ |
| `dependsOn` SUCCESS | ❌ | ✅ |
| `essential` フラグ | ❌ | ✅ (essential コンテナ終了でタスク停止) |
| `secrets` (ARN 参照) | ❌ | ✅ (ローカルマッピングファイルで解決) |
| メタデータエンドポイント | △ (別コンテナ必要) | ✅ (組み込み) |
| クレデンシャル | △ (別コンテナ必要) | ✅ (組み込み) |
| ボリュームマウント | ✅ | ✅ |

### 開発者体験

| 機能 | `ecs-cli local` | **Lecs** |
|------|------------------|----------|
| `validate` (静的解析) | ❌ | ✅ |
| `init` (プロジェクト生成) | ❌ | ✅ |
| `watch` (ファイル監視＋自動再起動) | ❌ | ✅ |
| `diff` (タスク定義の意味的比較) | ❌ | ✅ |
| `inspect` (実行中タスクの詳細表示) | ❌ | ✅ |
| `stats` (リソース使用量) | ❌ | ✅ |
| `history` (実行履歴) | ❌ | ✅ |
| ローカルオーバーライド | ❌ (Docker Compose override で代替) | ✅ (タスク定義を変更せずオーバーライド) |
| プロファイル (dev/staging) | ❌ | ✅ |
| ドライラン | ❌ | ✅ |
| 構造化イベント (NDJSON) | ❌ | ✅ |
| シェル補完 | ❌ | ✅ (bash/zsh/fish) |

---

## セットアップの比較

### `ecs-cli local` でメタデータ付きローカル実行する場合

```bash
# 1. ecs-cli をインストール
# 2. docker-compose をインストール
# 3. amazon-ecs-local-container-endpoints イメージを取得
docker pull amazon/amazon-ecs-local-container-endpoints

# 4. ecs-local-endpoints 用のネットワークを手動構築
docker network create --driver bridge \
  --subnet 169.254.170.0/24 \
  --gateway 169.254.170.1 \
  ecs-local-network

# 5. endpoints コンテナを起動
docker run -d --name ecs-local-endpoints \
  --network ecs-local-network \
  --ip 169.254.170.2 \
  -v /var/run:/var/run \
  -v $HOME/.aws:/home/.aws \
  amazon/amazon-ecs-local-container-endpoints

# 6. タスク定義を Docker Compose に変換
ecs-cli local create --task-def-file task-definition.json

# 7. 生成された Docker Compose ファイルを編集して
#    endpoints ネットワークに接続する設定を追加

# 8. 起動
ecs-cli local up
```

### Lecs でメタデータ付きローカル実行する場合

```bash
# 1. lecs をインストール (単一バイナリ)
# 2. 実行
lecs run -f task-definition.json
```

**以上。** メタデータもクレデンシャルも自動で提供される。

---

## なぜ `ecs-cli local` とアプローチが違うのか

`ecs-cli local` は既存の Docker Compose エコシステムを活用する設計。これは合理的だが、ECS タスク定義と Docker Compose の間に**意味論のギャップ**がある:

1. **dependsOn 条件**: ECS の `COMPLETE`/`SUCCESS` 条件は「コンテナが終了すること」を意味するが、Docker Compose の `depends_on` は「コンテナが起動すること」しか表現できない
2. **essential フラグ**: ECS では essential コンテナの終了がタスク全体の停止を意味するが、Docker Compose にこの概念はない
3. **secrets**: ECS の secrets は ARN 参照で、Secrets Manager/SSM Parameter Store から値を取得する。Docker Compose には直接の対応がない

Lecs は Docker Compose を経由せず、ECS タスク定義を**ネイティブに解釈**することで、これらのギャップを解消している。

---

## 他のツールとの関係

```
開発フロー:

  コード編集 → lecs run (ローカル確認) → git push → CI → copilot deploy (AWS)
               ^^^^^^^^^^^^^^^^^^^^^^^                    ^^^^^^^^^^^^^^^^^^^^^^^
               Lecs の領域                                Copilot の領域

  ecs-cli local は Lecs と同じ領域をカバーしていたが、
  Docker Compose 経由のアプローチで ECS 機能の再現に限界があった。
  そして 2023年に非推奨 (deprecated) になった。
```

### まとめ

- **`ecs-cli local`**: タスク定義 → Docker Compose に「翻訳」→ 一部の意味が失われる
- **Lecs**: タスク定義を「そのまま解釈」→ ECS の意味論を保持
- **Lecs はさらに**: バリデーション、watch、diff、inspect、stats、history など開発者向け機能を提供
