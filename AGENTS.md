<!-- skill-commons:start (generated, do not edit; run onboard.sh) -->
本專案使用 **skill-commons** 開發流程框架(以 git submodule 掛載於 `.agent/skills/_shared/`)。

任何開發任務(PRD、技術 spec、QA 方案、功能實作、除錯、code review),**必須先讀取 `.agent/skills/_shared/skill-router/SKILL.md`**,並依其「任務類型判斷表」載入對應 skill,再開始動作。

session 開始時請先執行 `bash .agent/skills/_shared/bootstrap/check.sh` 確認 bootstrap 設定正確;若出現警告,依指示修正後再繼續。

domain 專屬模式(檔案組織、命名、API 慣例)見 `.agent/project-manifest.md` 指向的 domain skills。

<!-- skill-commons-stamp: b661d6a3d86c -->
<!-- skill-commons:end -->
