# Egret Dog Routing Examples

AWS ECS タスク定義のサンプルを使った Egret のスモークテスト。

## ディレクトリ構成

```
examples/
├── aws-samples/          # aws-samples/aws-containers-task-definitions から取得
│   ├── nginx-fargate.json
│   ├── nginx-ec2.json
│   ├── consul-server.json
│   ├── consul-client.json
│   ├── wildfly-fargate.json
│   ├── tomcat-fargate.json
│   ├── gunicorn-fargate.json
│   └── kibana-fargate.json
│
├── multi-container/      # dependsOn + healthCheck + secrets
│   ├── task-definition.json
│   ├── egret-override.json
│   └── secrets.local.json
│
└── sidecar/              # sidecar pattern + volumes
    ├── task-definition.json
    └── egret-override.json
```

## 実行方法

### 全レベル一括実行

```bash
make dog-routing
# または
./examples/run-smoke-test.sh
```

### テストレベル

| Level | 内容 | Docker |
|-------|------|--------|
| 1 | `egret validate` でパース・バリデーション | 不要 |
| 2 | `egret run --dry-run` で設定確認 | 不要 |
| 3 | `egret run` で実際にコンテナ起動 | 必要 |

Level 3 は Docker が利用できない環境では自動スキップされます。

### ビルド済みバイナリを使う場合

```bash
EGRET_BIN=./target/release/egret ./examples/run-smoke-test.sh
```

## AWS 公式サンプルについて

`aws-samples/` のファイルは [aws-samples/aws-containers-task-definitions](https://github.com/aws-samples/aws-containers-task-definitions) からそのまま取得しています。

これらには Egret が解釈しないフィールド（`networkMode`, `requiresCompatibilities`, `logConfiguration`, `dockerLabels`, `ulimits` 等）が含まれており、パーサーの互換性テストとして機能します。

## 複合シナリオ

### multi-container

3コンテナ構成（postgres + redis + app）で以下を検証:
- `dependsOn` による起動順序制御
- `healthCheck` によるヘルスチェック
- `secrets` による環境変数注入

### sidecar

2コンテナ構成（app + log-router）で以下を検証:
- `essential: false` のサイドカーコンテナ
- `dependsOn` の `START` condition
- 共有ボリューム (`volumes` + `mountPoints`)
- `firelensConfiguration` の無視
