# P2 — L5 Input Engineering

**分期**: P2 ・ 依賴: P1 ・ 估時: ~4h ・ 上層: `spec-overview.md`

> 把原始中文/中英混合輸入收斂成 `NormalizedInput`（intent + slots + confidence + warnings）。純函式、stage 化（由 P1 的 registry 依 config 組裝啟用順序）。

## 變更檔案
| 路徑 | 操作 | 說明 |
|------|------|------|
| `src/runtime/input/mod.rs` | NEW | input 子模組匯出 |
| `src/runtime/input/normalizer.rs` | NEW | NFKC **＋ 全形標點對照表** + whitespace + case |
| `src/runtime/input/intent.rs` | NEW | option-path + lexicon 計分 + text override + 信心分級 |
| `src/runtime/input/slots.rs` | NEW | extractor registry；**asset 走 config allowlist** |
| `src/runtime/input/pipeline.rs` | NEW | `run_rule_pipeline`（依組裝出的 stage 執行）|

## 邏輯

### normalize（`normalizer.rs`，對照 `input-normalizer.ts`）
- NFKC（`unicode-normalization`）**＋ 手工全形標點對照表**（`「」『』→ "`、`、→ ,` 等；NFKC 不轉這些 CJK 標點，必須連表移植）+ 收斂空白 + 大小寫。

### intent 分類（`intent.rs`，對照 `intent-classifier.ts` + `lexicon.ts`）
```text
1. `option_id` 命中 option_prefixes → intent=mapped, confidence=option_path, source=option-path
   - `option_id` 未命中 prefix 不回 400；加入 warning + audit，退回文字 lexicon / unknown。這保留舊 client 相容性並讓 migration 可逐步送出 option ids。
2. 文字 lexicon 計分（長字權重 keyword_long_weight / 短字 short）；依 margin_tiers 給 confidence；
   無命中 → unknown(unknown_confidence)
3. text override（D3）：文字信心 >= text_override_confidence → 覆蓋 option-path，source=text-override，保留兩 candidate
```

### slots（`slots.rs`，對照 `slot-extractor.ts`）
- extractor registry：`time_range` / `metric` / `asset` / `rank_limit`，由 `[runtime.slots] extractors` 選用。
- **asset 未知判定改 config allowlist**（`RuntimeConfig.asset_allowlist`），不得硬編特定資產名；未知 asset → warning。

### pipeline（`pipeline.rs`）
- `run_rule_pipeline`：依 registry 組裝出的 `[runtime.pipeline].input_stages` 依序執行 `normalize → input_guard → injection → intent → slots`（input_guard / injection 屬 P3，先預留串接點）→ 產 `NormalizedInput` 並對 allowlist 驗證 intent。

## 測試（移植自 `run-input-pipeline.test.ts`）
```rust
#[test] fn site_build_six_months() { /* "近六個月站點建置" → intent=site-build, months=Some(6) */ }
#[test] fn option_id_maps_to_option_path() { /* option_id=charging.* → charging intent, source=OptionPath */ }
#[test] fn text_override_beats_option_path() { /* option_id=charging + "這個月營收賺多少" → revenue, source=TextOverride */ }
#[test] fn unknown_asset_warns() { /* "Zeta...": asset 不在 config allowlist → warning（asset 走 config，非硬編） */ }
#[test] fn normalizer_maps_fullwidth_punctuation() { /* 「」、 等 → 對照表結果（NFKC 不足） */ }
```
> parity 前提：`thresholds.toml` 的完整 `[classifier]` 區塊（見 P1）必須就位，否則 text-override/confidence 案例寫不出。
