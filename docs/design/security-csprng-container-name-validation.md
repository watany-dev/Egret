# セキュリティ修正設計書: CSPRNG トークン生成 + コンテナ名バリデーション

## 概要

セキュリティレビューで指摘された VULN-001 / VULN-004 を修正する。

---

## 1. VULN-001: CSPRNG（getrandom）によるトークン生成へ置換

### 1.1 問題

現在の `generate_auth_token()` は `RandomState`（SipHash）+ `SystemTime` を使用しているが、
これは **CSPRNG ではない**。

```rust
// 現在の実装 (src/metadata/mod.rs:288-308)
pub fn generate_auth_token() -> String {
    let state = RandomState::new();        // SipHash — 非暗号学的
    let mut hasher = state.build_hasher();
    hasher.write_u128(SystemTime::now()... // タイムスタンプ — 推測可能
    ...
}
```

**リスク:**
- `RandomState` の内部シードは OS 乱数源から取得されるが、SipHash 出力は暗号学的にランダムではない
- `SystemTime` はナノ秒精度でも、同一ホスト上のプロセスから推測可能
- ローカルホスト限定とはいえ、認証トークンは CSPRNG で生成すべき（Defense in Depth）

### 1.2 修正方針

`getrandom` クレートで OS 提供の CSPRNG から直接バイト列を取得し、hex エンコードする。

### 1.3 なぜ `getrandom` か

| 観点 | 判断 |
|---|---|
| **既存依存** | `getrandom` v0.2.17, v0.3.4 が Cargo.lock に存在（`aws-config`, `bollard` 等の推移的依存） |
| **ライセンス** | MIT OR Apache-2.0 — `deny.toml` の許可リストに適合 |
| **最小依存方針** | 推移的依存の直接利用のため実質的に新規依存なし |
| **`rand` vs `getrandom`** | `rand` は汎用乱数生成ライブラリで過剰。トークン生成にはバイト列取得のみで十分 |
| **CSPRNG 保証** | Linux: `getrandom(2)` syscall, macOS: `CCRandomGenerateBytes`, Windows: `BCryptGenRandom` |

### 1.4 変更詳細

#### `Cargo.toml`

```toml
[dependencies]
# ... existing ...
getrandom = { version = "0.3", features = ["std"] }
```

**バージョン選定:** v0.3 系を採用。Cargo.lock に v0.3.4 が既に存在し、推移的依存と統合される。
`features = ["std"]` で `std::error::Error` 実装を有効化し、`getrandom::Error` を `anyhow` と統合可能にする。

#### `src/metadata/mod.rs` — `generate_auth_token()`

**Before:**
```rust
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

**After:**
```rust
/// Generate an authorization token for the credentials endpoint.
///
/// Uses the OS-provided CSPRNG via `getrandom` to produce a 128-bit
/// cryptographically random hex token (32 hex characters).
pub fn generate_auth_token() -> String {
    let mut buf = [0u8; 16];
    // OS CSPRNG failure is unrecoverable — no fallback possible.
    #[allow(clippy::expect_used)]
    getrandom::fill(&mut buf).expect("OS CSPRNG unavailable");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}
```

**設計判断:**

| 項目 | 決定 | 理由 |
|---|---|---|
| トークン長 | 16 bytes = 128 bits | 2^128 の探索空間。ローカルホスト認証に十分 |
| エンコード | hex (32文字) | 既存トークン長と互換、ECS `Authorization` ヘッダーに安全 |
| エラー処理 | `#[allow(clippy::expect_used)]` + `expect` | OS CSPRNG が利用不能な環境はサポート対象外。プロジェクト方針上 `expect_used = "warn"` かつ `-D warnings` で全 warn がエラーに昇格されるため、`#[allow]` で明示的に許可する。本番コードで `expect` を使用する唯一の箇所であり、CSPRNG 不可はプロセス実行不能を意味するため正当化される |
| `SystemTime` 除去 | 完全除去 | CSPRNG のみで一意性・予測不能性を確保。タイムスタンプ混合は不要 |

### 1.5 テスト計画

既存テストをそのまま維持し、CSPRNG 特性の追加テストを加える:

| テストケース | 期待結果 | 変更 |
|---|---|---|
| `generate_auth_token_is_nonempty_hex` | 32文字hex | 既存維持 |
| `generate_auth_token_is_unique` | 2回呼び出しで異なる値 | 既存維持 |
| `generate_auth_token_length_is_consistent` | 100回呼び出しで全て32文字 | **新規** |

### 1.6 影響範囲

- `src/metadata/mod.rs` — `generate_auth_token()` 関数の実装置換
- `Cargo.toml` — `getrandom` 依存追加
- 他のファイルへの影響: なし（関数シグネチャ `fn generate_auth_token() -> String` は不変）

---

## 2. VULN-004: コンテナ名の正規表現バリデーション追加

### 2.1 問題

現在の `validate()` はコンテナ名に対して空文字チェックのみを行っている:

```rust
// src/taskdef/mod.rs:418-423
for (i, container) in self.container_definitions.iter().enumerate() {
    if container.name.is_empty() {
        return Err(TaskDefError::Validation(format!(
            "container name must not be empty at index {i}"
        )));
    }
```

**リスク:**
- ECS 非互換の名前（スペース、特殊文字、256文字以上）を許容
- Docker コンテナ名として `{family}-{name}` 形式で使用されるため、不正文字はランタイムエラーを引き起こす
- パストラバーサルやインジェクションに繋がる文字列（`../`, `;`, `$()` 等）がコンテナ名として渡される可能性

### 2.2 ECS 仕様

AWS ECS の `containerDefinitions[].name` フィールドの制約:

- パターン: `[a-zA-Z0-9_-]+`
- 最大長: 255 文字
- 最小長: 1 文字（空文字禁止）

正規表現として: `^[a-zA-Z0-9_-]{1,255}$`

### 2.3 修正方針

`regex` クレートを追加せず、`char` レベルの検証で ECS 互換バリデーションを実装する。

### 2.4 なぜ `regex` を使わないか

| 観点 | 判断 |
|---|---|
| パターンの複雑さ | `[a-zA-Z0-9_-]` は `char::is_ascii_alphanumeric()` + `_` + `-` で表現可能 |
| 依存最小化 | `regex` クレートはコンパイルサイズが大きく、この用途には過剰 |
| パフォーマンス | 文字単位の検証は正規表現エンジンより高速 |

### 2.5 変更詳細

#### `src/taskdef/mod.rs`

**A) バリデーション関数を追加:**

```rust
/// Maximum length for a container name (ECS specification).
const MAX_CONTAINER_NAME_LEN: usize = 255;

/// Validate that a container name matches ECS naming rules: `[a-zA-Z0-9_-]{1,255}`.
fn validate_container_name(name: &str, index: usize) -> Result<(), TaskDefError> {
    if name.is_empty() {
        return Err(TaskDefError::Validation(format!(
            "container name must not be empty at index {index}"
        )));
    }
    if name.len() > MAX_CONTAINER_NAME_LEN {
        return Err(TaskDefError::Validation(format!(
            "container name must not exceed {MAX_CONTAINER_NAME_LEN} characters at index {index}, \
             got {} characters: '{name}'",
            name.len()
        )));
    }
    if let Some(pos) = name.find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-') {
        // `pos` is a byte offset from `find`, but all valid chars are ASCII (1 byte each),
        // so byte offset == char index for the first invalid character.
        // Use `&name[pos..]` to safely extract the character at the byte offset.
        let invalid_char = name[pos..].chars().next().unwrap_or('?');
        return Err(TaskDefError::Validation(format!(
            "container name contains invalid character '{invalid_char}' at position {pos} \
             in '{name}' at index {index} (allowed: a-z, A-Z, 0-9, _, -)"
        )));
    }
    Ok(())
}
```

**設計判断:**

| 項目 | 決定 | 理由 |
|---|---|---|
| 最大長 | 255 | ECS 仕様準拠 |
| 許可文字 | `a-zA-Z0-9_-` | ECS 仕様準拠 |
| エラーメッセージ | 不正文字の位置と値を表示 | デバッグ容易性 |
| `find` vs `all` | 最初の不正文字で即座にエラー | Fail-fast 方針 |

**B) `validate()` メソッドの修正:**

```rust
fn validate(&self) -> Result<(), TaskDefError> {
    // ... family / containerDefinitions empty checks ...

    for (i, container) in self.container_definitions.iter().enumerate() {
        // REPLACED: empty check → full ECS name validation
        validate_container_name(&container.name, i)?;

        if container.image.is_empty() {
            // ... existing ...
        }
        // ... rest unchanged ...
    }
    // ...
}
```

既存の `is_empty()` チェックは `validate_container_name()` 内に統合されるため、
重複排除される。

### 2.6 テスト計画

| テストケース | 入力 | 期待結果 |
|---|---|---|
| 正常: 英数字 | `"app"` | OK |
| 正常: ハイフン | `"my-app"` | OK |
| 正常: アンダースコア | `"my_app"` | OK |
| 正常: 数字開始 | `"123app"` | OK |
| 正常: 最大長 | `"a" × 255` | OK |
| 異常: 空文字 | `""` | Validation エラー（既存テスト維持） |
| 異常: スペース含有 | `"my app"` | Validation エラー |
| 異常: ドット含有 | `"my.app"` | Validation エラー |
| 異常: スラッシュ含有 | `"my/app"` | Validation エラー |
| 異常: 日本語 | `"アプリ"` | Validation エラー |
| 異常: 256文字超過 | `"a" × 256` | Validation エラー（長さ超過） |
| 異常: 特殊文字 | `"app;rm -rf"` | Validation エラー |
| 異常: パストラバーサル | `"../escape"` | Validation エラー |

### 2.7 影響範囲

- `src/taskdef/mod.rs` — `validate()` メソッドの修正 + `validate_container_name()` 追加
- 既存テスト `error_empty_container_name` — エラーメッセージは変わらないため互換維持
- `src/taskdef/diagnostics.rs` — 変更不要（diagnostics はバリデーション通過後に実行）

---

## 実装順序

1. **VULN-004（コンテナ名バリデーション）を先に実装**
   - 変更範囲: `src/taskdef/mod.rs` のみ
   - 既存テストへの影響: なし（既存テストは全て `[a-zA-Z0-9_-]` 準拠の名前を使用）
   - コミット単位: 1コミット

2. **VULN-001（CSPRNG トークン生成）を実装**
   - 変更範囲: `Cargo.toml`, `src/metadata/mod.rs`
   - 既存テストへの影響: なし（関数シグネチャ不変）
   - コミット単位: 1コミット

### 理由

VULN-004 は `src/taskdef/mod.rs` のみの変更で独立性が高い。
VULN-001 は `Cargo.toml` への依存追加を伴うため、先に VULN-004 を完了させて
ベースラインの安定を確認した上で取り組む。

## 確認

各コミット後に `make check` を実行:
1. `fmt-check` — フォーマット準拠
2. `lint` — clippy 警告なし
3. `test` — 全テスト通過
4. `doc` — ドキュメント警告なし
5. `deny` — 依存関係監査通過
