# 脆弱性修正設計書: クレデンシャル認証 + パストラバーサル防止

## 概要

セキュリティレビューで発見された CRITICAL / HIGH 脆弱性2件を修正する。

---

## 1. CRITICAL: クレデンシャルエンドポイントの認証トークン実装

### 1.1 問題

`/credentials` エンドポイントが認証なしでAWSクレデンシャル（`AccessKeyId`, `SecretAccessKey`, `Token`）を返す。メタデータサーバーは `127.0.0.1` にバインドされているが、ローカルマシン上の任意のプロセスがポート番号を推測・取得しクレデンシャルを窃取できる。

ECS Task Metadata Endpoint V4 は `Authorization` ヘッダーによるトークン検証を要求するが、Lecsにはこの保護がない。

### 1.2 修正方針

ECS互換の認証方式を実装する:

1. サーバー起動時にランダムな認証トークンを生成
2. `ServerState` にトークンを保持
3. `/credentials` ハンドラで `Authorization` ヘッダーを検証
4. コンテナに `AWS_CONTAINER_AUTHORIZATION_TOKEN` 環境変数としてトークンを注入

### 1.3 変更詳細

#### `src/metadata/mod.rs`

**A) `ServerState` にフィールド追加:**

```rust
pub struct ServerState {
    pub task_metadata: TaskMetadata,
    pub container_metadata: HashMap<String, ContainerMetadata>,
    pub credentials: Option<AwsCredentials>,
    pub container_ids: HashMap<String, String>,
    pub auth_token: String,  // NEW
}
```

**B) トークン生成関数を追加:**

```rust
/// Generate an authorization token for the credentials endpoint.
///
/// Uses `RandomState` (SipHash with random keys) from the standard library
/// combined with the current timestamp to produce a unique hex token.
/// Cryptographic strength is not required here — the purpose is process-level
/// isolation on localhost, not network-level security.
pub fn generate_auth_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    let h1 = hasher.finish();

    let state2 = RandomState::new();
    let mut hasher2 = state2.build_hasher();
    hasher2.write_u64(h1);
    let h2 = hasher2.finish();

    format!("{h1:016x}{h2:016x}")
}
```

**設計判断: なぜ `rand` クレートを追加しないか**
- 目的はローカルプロセス間の分離（ネットワーク攻撃は `127.0.0.1` バインドで防御済み）
- `RandomState` は SipHash + OS提供のランダムシードを使用し、十分な一意性がある
- 依存クレート最小化のプロジェクト方針に従う

**C) `credentials_handler` に認証を追加:**

```rust
async fn credentials_handler(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let state = state.read().await;

    let authorized = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == state.auth_token);

    if !authorized {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    state.credentials.as_ref().map_or_else(
        || StatusCode::NOT_FOUND.into_response(),
        |creds| (StatusCode::OK, Json(serde_json::to_value(creds).ok())).into_response(),
    )
}
```

**D) 他のエンドポイント（`/health`, `/v4/...`）は認証不要のまま**
- ECS互換: メタデータエンドポイントは認証なしでアクセス可能
- クレデンシャルのみが機密情報

#### `src/cli/run.rs`

**A) `start_metadata_server` で `auth_token` を生成・返却:**

`ServerState` 構築時に `auth_token` フィールドを設定し、トークンを返す。

**B) `build_container_config` にトークンパラメータを追加:**

シグネチャ変更:
```rust
fn build_container_config(
    family: &str,
    def: &ContainerDefinition,
    network: &str,
    metadata_port: Option<u16>,
    volumes: &[Volume],
    auth_token: Option<&str>,  // NEW
) -> ContainerConfig
```

環境変数として注入:
```rust
if let Some(port) = metadata_port {
    // ... existing env vars ...
    if let Some(token) = auth_token {
        env.push(format!("AWS_CONTAINER_AUTHORIZATION_TOKEN={token}"));
    }
}
```

**C) `run_task` のシグネチャ変更:**

`auth_token` を受け取り `build_container_config` に伝搬:
```rust
pub async fn run_task(
    client: &(impl ContainerRuntime + ?Sized),
    task_def: &TaskDefinition,
    metadata_port: Option<u16>,
    auth_token: Option<&str>,  // NEW
) -> Result<(String, Vec<(String, String)>)>
```

### 1.4 テスト計画

| テストケース | 期待結果 |
|---|---|
| トークンなしで `/credentials` | 401 Unauthorized |
| 不正トークンで `/credentials` | 401 Unauthorized |
| 正しいトークンで `/credentials`（クレデンシャルあり） | 200 + JSON |
| 正しいトークンで `/credentials`（クレデンシャルなし） | 404 |
| `/health` はトークン不要 | 200 |
| `/v4/{name}` はトークン不要 | 200 |
| `/v4/{name}/task` はトークン不要 | 200 |
| `generate_auth_token()` が空でない32文字hex | 通過 |
| 2回呼び出しで異なるトークン生成 | 通過 |
| `build_container_config` に `auth_token` 渡した場合に環境変数に含まれる | 通過 |
| `build_container_config` に `None` 渡した場合に環境変数に含まれない | 通過 |

---

## 2. HIGH: ボリュームマウントのパストラバーサル検証

### 2.1 問題

`host.source_path` と `container_path` に対して空文字チェックのみで、以下が欠如:
- 絶対パス検証（`/` で始まるか）
- パストラバーサル検出（`..` コンポーネントの禁止）

悪意あるタスク定義でホストの任意ディレクトリをコンテナにマウント可能。

### 2.2 修正方針

`validate_mount_points()` 内でパスの安全性を検証する。

### 2.3 変更詳細

#### `src/taskdef/mod.rs`

**A) パス検証ヘルパー関数を追加:**

```rust
/// Validate that a path is absolute and does not contain parent directory traversal.
fn validate_path_safety(path: &str, field_name: &str, context: &str) -> Result<(), TaskDefError> {
    if !path.starts_with('/') {
        return Err(TaskDefError::Validation(format!(
            "{context}: {field_name} must be an absolute path, got '{path}'"
        )));
    }
    for component in std::path::Path::new(path).components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(TaskDefError::Validation(format!(
                "{context}: {field_name} must not contain '..' path traversal, got '{path}'"
            )));
        }
    }
    Ok(())
}
```

**設計判断:**
- `std::path::Path::components()` を使用し、OS固有のパス解析に委ねる
- `ParentDir` (`..`) のみを拒否し、`CurDir` (`.`) は許容（無害）
- シンボリックリンクのチェックは行わない（Docker自体がマウント時に解決するため）

**B) `validate_mount_points()` に組み込み:**

既存の空文字チェックの直後にパス安全性検証を追加:

```rust
fn validate_mount_points(&self) -> Result<(), TaskDefError> {
    let volume_names: HashSet<&str> = self.volumes.iter().map(|v| v.name.as_str()).collect();

    for container in &self.container_definitions {
        for mp in &container.mount_points {
            if !volume_names.contains(mp.source_volume.as_str()) { /* existing */ }
            if mp.container_path.is_empty() { /* existing */ }

            // NEW: validate container_path safety
            validate_path_safety(
                &mp.container_path,
                "containerPath",
                &format!("container '{}', volume '{}'", container.name, mp.source_volume),
            )?;
        }
    }

    for volume in &self.volumes {
        if let Some(host) = &volume.host {
            if host.source_path.is_empty() { /* existing */ }

            // NEW: validate source_path safety
            validate_path_safety(
                &host.source_path,
                "host.sourcePath",
                &format!("volume '{}'", volume.name),
            )?;
        }
    }

    Ok(())
}
```

### 2.4 テスト計画

| テストケース | 期待結果 |
|---|---|
| `source_path: "../../../etc"` | Validation エラー（`..` 検出） |
| `source_path: "relative/path"` | Validation エラー（絶対パスでない） |
| `source_path: "/safe/../../escape"` | Validation エラー（`..` 検出） |
| `container_path: "../escape"` | Validation エラー |
| `container_path: "relative"` | Validation エラー |
| `source_path: "/absolute/safe"` | 成功 |
| `container_path: "/app/data"` | 成功 |
| `source_path: "/path/with/./dot"` | 成功（`.` は無害） |

---

## 実装順序

1. **HIGH（パストラバーサル）を先に実装**
   - 変更範囲: `src/taskdef/mod.rs` のみ
   - 既存テストへの影響: なし（既存テストは全て絶対パスを使用）
   - コミット単位: 1コミット

2. **CRITICAL（認証トークン）を実装**
   - 変更範囲: `src/metadata/mod.rs`, `src/cli/run.rs`
   - 既存テストへの影響: `ServerState` 構築箇所の修正が必要
   - コミット単位: 1コミット

## 確認

各コミット後に `make check` を実行:
1. `fmt-check` — フォーマット準拠
2. `lint` — clippy警告なし
3. `test` — 全テスト通過
4. `doc` — ドキュメント警告なし
5. `deny` — 依存関係監査通過
