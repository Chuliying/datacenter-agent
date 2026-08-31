# Falcon 維運詢問：permissions endpoint transport 與 403 semantics

Status: Pending

Related follow-up: FU-005（Blocking）

## 可直接轉發內容

**主旨：**【請確認】Falcon permissions endpoint 的 browser token transport 與 403 semantics

Hi Falcon 維運團隊，

我們正在將 Falcon dashboard 使用者的 access token 由 BFF 傳給 runtime，再由 runtime 呼叫
`GET /api/auth/me/permissions`。為避免在技術 spec 前鎖定錯誤契約，請協助書面確認：

1. `GET /api/auth/me/permissions` 是否接受正常瀏覽器登入取得的 `access_token`，以
   `Authorization: Bearer <token>` 呼叫？指南中的範例 token 由 `external-login` 產生，且帶
   `platform=api_client`；我們需要確認 browser-issued token 是否也被接受。若不接受，請提供
   支援的 transport、必要 headers，以及 `platform` / `audience` 要求。

2. 請提供該端點完整的 403 語義清單，包含：
   - 所有可能的 error code / response body；
   - `user_inactive`、`user_blocklisted` 是否涵蓋全部 403 情況；
   - 哪些情況可透過 refresh 修復，哪些情況明確不可 refresh；
   - 是否存在暫時性或權限範圍型 403。

如方便，請附目前 production / staging 版本與去識別化的 response examples。我們不需要任何
credential value。

謝謝。

## 傳送備註

- 請回覆完整契約與代表性 response，不要在郵件中放 access token、cookie 或 API key。
- 在兩項確認取得前，FU-005 維持 Blocking，PRD 不交付 spec。
