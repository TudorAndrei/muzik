# Changelog
All notable changes to this project will be documented in this file. See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

- - -
## v2.6.0 - 2026-09-29
#### Features
- skip albums already in the library unless the duplicates setting says otherwise - (d396305) - TudorAndrei
#### Miscellaneous Chores
- (**homebrew**) update the cask to v2.5.0 - (5aa295a) - TudorAndrei

- - -

## v2.5.0 - 2026-09-29
#### Features
- (**cli**) add jobs and watchlist commands on the shared queue - (7d126d1) - TudorAndrei
- (**gui**) run jobs from queues with download, process and import gates - (75c8f7b) - TudorAndrei
- (**gui**) park watchlist choices and resume them from the job queue - (3b77116) - TudorAndrei
- (**jobs**) add a runner lock and cancel requests across processes - (e6177a4) - TudorAndrei
- (**jobs**) claim queues in priority order and find open jobs of an item - (c98a563) - TudorAndrei
- (**jobs**) claim from several queues and cancel open jobs - (106eede) - TudorAndrei
- (**jobs**) reopen a resumed job so its question stays - (f778ed9) - TudorAndrei
- (**jobs**) add a SQLite job queue for background work - (de11143) - TudorAndrei
- (**soulseek**) share one login between all jobs - (17d0d50) - TudorAndrei
- (**watchlist**) sync playlists apart from item runs and write the file in one step - (788acc2) - TudorAndrei
- (**watchlist**) park items that wait for a choice - (4ff588b) - TudorAndrei
#### Bug Fixes
- (**import**) identify album match choices by release ID - (68f705e) - TudorAndrei
- (**library**) wait for a busy beets database instead of failing - (9cc6842) - TudorAndrei
- (**watchlist**) record finished stages of a waiting item - (59d32fc) - TudorAndrei
#### Documentation
- describe the shared job queue for the CLI and the app - (b37352a) - TudorAndrei
#### Refactoring
- move the job queue runtime into a shared muzik-runner crate - (b54ffe1) - TudorAndrei
- use strum enums for fixed string choices - (f7b3168) - TudorAndrei
#### Miscellaneous Chores
- (**homebrew**) update the cask to v2.4.1 - (2f8b065) - TudorAndrei

- - -

## v2.4.1 - 2026-09-29
#### Bug Fixes
- (**gui**) make Recent events readable - (4b76349) - TudorAndrei
#### Miscellaneous Chores
- (**homebrew**) update the cask to v2.4.0 - (95c2ae5) - TudorAndrei

- - -

## v2.4.0 - 2026-09-29
#### Features
- (**agent**) choose album matches and Soulseek downloads with Codex - (caa8819) - TudorAndrei
- (**gui**) show match scores and the AI suggestion in decisions - (763c0cc) - TudorAndrei
#### Miscellaneous Chores
- (**homebrew**) update the cask to v2.3.1 - (109ec37) - TudorAndrei

- - -

## v2.3.1 - 2026-09-29
#### Bug Fixes
- (**gui**) clear the playlist field after add and the decision after a job - (649c100) - TudorAndrei
- (**watchlist**) read every playlist before processing items - (8eff9e9) - TudorAndrei

- - -

## v2.3.0 - 2026-09-29
#### Features
- (**watchlist**) report each save during a refresh - (1f34aab) - TudorAndrei
#### Bug Fixes
- (**gui**) show decision choices first and reload the watchlist during jobs - (8eeee1b) - TudorAndrei
#### Miscellaneous Chores
- (**homebrew**) update the cask to v2.2.1 - (5e130ad) - TudorAndrei

- - -

## v2.2.1 - 2026-09-29
#### Bug Fixes
- (**gui**) quit with Command-Q and when the window closes - (de6a033) - TudorAndrei
#### Miscellaneous Chores
- (**homebrew**) update the cask to v2.2.0 - (2031708) - TudorAndrei

- - -

## v2.2.0 - 2026-09-29
#### Features
- (**gui**) show services as a compact list - (452ca5c) - TudorAndrei
- (**gui**) configure the Soulseek account in Settings - (b79165c) - TudorAndrei
#### Bug Fixes
- (**homebrew**) use postflight_steps in the cask - (9291742) - TudorAndrei
#### Miscellaneous Chores
- (**homebrew**) set the v2.1.0 app hash in the cask - (53d71ff) - TudorAndrei

- - -

## v2.1.0 - 2026-09-29
#### Features
- (**cli**) remove the install-app and gui commands - (0f1eb4e) - TudorAndrei
- (**gui**) find Homebrew and mise tools when opened from Finder - (dcd7561) - TudorAndrei
- (**release**) publish the CLI and Muzik.app as separate downloads - (5e91ba7) - TudorAndrei
#### Bug Fixes
- (**ci**) build Intel macOS releases and publish existing tags - (2fa6d5a) - TudorAndrei
#### Continuous Integration
- build releases only for macOS arm64 and Linux x86_64 - (ae2c32b) - TudorAndrei

- - -

## v2.0.0 - 2026-09-29
#### Features
- (**branding**) switch to waveform logo - (9fe75e3) - TudorAndrei
- (**branding**) add muzik logo and desktop icon - (c6edc18) - TudorAndrei
- (**cli**) use shared Beets and playlist services - (8b67dbe) - TudorAndrei
- (**cli**) add Rust split archive and workflow commands - (a5bdb70) - TudorAndrei
- (**cli**) organize beets library files in Rust - (adff165) - TudorAndrei
- (**cli**) install macOS app bundle from Rust - (311ff0b) - TudorAndrei
- (**cli**) save Spotify watches in Rust - (60c6e4d) - TudorAndrei
- (**config**) store GUI settings in Rust - (a0903b4) - TudorAndrei
- (**core**) share config import and workflow services - (c2c458c) - TudorAndrei
- (**core**) parse saved audio chapters in Rust - (8063fb0) - TudorAndrei
- (**core**) reconcile saved watchlists in Rust - (70e762e) - TudorAndrei
- (**core**) add muzik-core types and beets config loading - (62f8404) - TudorAndrei
- (**gui**) remove saved config summary from Workflow - (2a39a1e) - TudorAndrei
- (**gui**) merge Config and Settings into one tab - (0ebd70d) - TudorAndrei
- (**gui**) use kit tables, alerts and inputs on remaining pages - (c680b85) - TudorAndrei
- (**gui**) apply muzik theme and kit components to chrome and watchlist - (bc17ded) - TudorAndrei
- (**gui**) run local audio jobs in Rust - (602c109) - TudorAndrei
- (**gui**) validate workflow requests in Rust - (02839d3) - TudorAndrei
- (**gui**) load and check watchlists in Rust - (01cf75a) - TudorAndrei
- (**gui**) edit saved watchlist in Rust - (83d015f) - TudorAndrei
- (**gui**) cache watchlist thumbnails in Rust - (af6ae5e) - TudorAndrei
- (**gui**) check external services in Rust - (29a0bd3) - TudorAndrei
- (**gui**) start with Rust backend and defer Python service - (f3c93a0) - TudorAndrei
- (**gui**) scan downloaded audio in Rust - (fa03708) - TudorAndrei
- (**gui**) add separate saved config window - (5972823) - TudorAndrei
- (**gui**) persist workflow configuration - (09439b0) - TudorAndrei
- (**gui**) group watchlist item commands - (f975fee) - TudorAndrei
- (**gui**) show Spotify setup by account state - (5620259) - TudorAndrei
- (**gui**) show library and service check states - (d34df17) - TudorAndrei
- (**gui**) restore watchlist and choice controls - (7429dbe) - TudorAndrei
- (**gui**) show structured workflow activity - (e90e14c) - TudorAndrei
- (**gui**) group workflow with GPUI Kit controls - (911b7f1) - TudorAndrei
- (**gui**) merge thumbnail events into visible cards - (dc7ce63) - TudorAndrei
- (**gui**) cache thumbnails outside workflow jobs - (c59b9d4) - TudorAndrei
- (**gui**) show readable decisions and ignore stale reads - (7e8b852) - TudorAndrei
- (**gui**) load visible thumbnails automatically - (d2429a9) - TudorAndrei
- (**gui**) show library details and saved Spotify sources - (db01ec5) - TudorAndrei
- (**gui**) add GPUI Kit desktop frontend - (b86550a) - TudorAndrei
- (**gui**) route desktop command to GPUI app - (565b572) - TudorAndrei
- (**gui**) add quality and Seakarr workflow controls - (354ea05) - TudorAndrei
- (**gui**) clarify Beets match choices - (f76fcfa) - TudorAndrei
- (**gui**) add compact top navigation - (43eb197) - TudorAndrei
- (**gui**) add the YouTube-style watchlist viewer - (b1eabe3) - TudorAndrei
- (**import**) honor incremental and autotag options - (9b09bc7) - TudorAndrei
- (**import**) run imports through the native pipeline - (e931e44) - TudorAndrei
- (**import**) sync release tracks and singletons - (7d12b3e) - TudorAndrei
- (**import**) plan and apply imports without beets - (8d75636) - TudorAndrei
- (**import**) plan album matches and find duplicates - (8244573) - TudorAndrei
- (**import**) render beets path formats - (7ccf7fe) - TudorAndrei
- (**import**) rank beets candidates with the native matcher - (72c8995) - TudorAndrei
- (**import**) compare native album ranking in shadow mode - (7004198) - TudorAndrei
- (**import**) evaluate beets path templates - (a7ee417) - TudorAndrei
- (**import**) add file operations for library placement - (85cd852) - TudorAndrei
- (**library**) write items and albums to the beets database - (f45578e) - TudorAndrei
- (**library**) serve library lookups from the native reader - (6849199) - TudorAndrei
- (**library**) parse and run beets queries - (719ce75) - TudorAndrei
- (**library**) read beets library items and albums - (eff151c) - TudorAndrei
- (**match**) add track assignment and candidate ranking - (728bb46) - TudorAndrei
- (**match**) port track and album distance scoring - (44a2f8c) - TudorAndrei
- (**match**) port beets string distance - (c1212e3) - TudorAndrei
- (**metadata**) look up chapter tracklists with the native client - (4753cf6) - TudorAndrei
- (**metadata**) add rate-limited MusicBrainz release client - (d189748) - TudorAndrei
- (**native-gui**) add JSON-lines backend service - (fd6067e) - TudorAndrei
- (**quality**) add measured audio quality decisions - (8ac478c) - TudorAndrei
- (**rust**) add native CLI and root Cargo workspace - (3143cc6) - TudorAndrei
- (**seakarr**) decode Soulseek file-attribute quality codes - (b41c21c) - TudorAndrei
- (**seakarr**) add the embedded Soulseek bridge - (13b9d7d) - TudorAndrei
- (**soulseek**) download selected results in Rust - (e409de0) - TudorAndrei
- (**soulseek**) search and rank peers in Rust - (f5e8c34) - TudorAndrei
- (**soulseek**) check connection in Rust CLI - (bc1a7a0) - TudorAndrei
- (**soulseek**) report low-quality library tracks and Soulseek replacements - (1465f10) - TudorAndrei
- (**spotify**) export playlist metadata in Rust - (3b7ec2a) - TudorAndrei
- (**spotify**) list account playlists in Rust - (13f3289) - TudorAndrei
- (**spotify**) run browser login in Rust - (17697f2) - TudorAndrei
- (**spotify**) check account status in Rust - (c805e07) - TudorAndrei
- (**spotify**) move settings and logout to Rust - (0d9a42e) - TudorAndrei
- (**spotify**) carry current playlist work into GPUI branch - (54d8af2) - TudorAndrei
- (**spotify**) acquire tracks directly with Seakarr - (80b0d3b) - TudorAndrei
- (**tags**) probe audio and embed cover art natively - (05d4510) - TudorAndrei
- (**tags**) read and write beets-compatible tags with lofty - (22f0bb6) - TudorAndrei
- (**watchlist**) read and update saved state in Rust - (f0abee4) - TudorAndrei
- (**watchlist**) match already-organized albums by exact video id - (0781192) - TudorAndrei
- (**watchlist**) detect albums already organized in Beets - (e87c68d) - TudorAndrei
- (**watchlist**) show youtube-style item controls - (7bcf9ec) - TudorAndrei
- (**watchlist**) add cached thumbnails and item actions - (dcf80d1) - TudorAndrei
- (**watchlist**) process only pending playlist videos - (f7ac3e7) - TudorAndrei
- (**watchlist**) store YouTube playlist items and state - (966b7a6) - TudorAndrei
- (**workflow**) replace Python jobs with Rust services - (20a387e) - TudorAndrei
- (**workflow**) add YouTube-first quality upgrades - (20cbf43) - TudorAndrei
#### Bug Fixes
- (**beets**) fall back during MusicBrainz outages - (cd5294e) - TudorAndrei
- (**build**) lock GUI Soulseek dependency - (715949c) - TudorAndrei
- (**ci**) install GPUI Linux libraries with a fresh apt index - (9d120ca) - TudorAndrei
- (**ci**) run the release bump without hiding cog errors - (c4fe523) - TudorAndrei
- (**ci**) install ffmpeg for Linux tests - (8c352d6) - TudorAndrei
- (**cli**) bundle native app binaries and find bandsnatch - (72ed0b1) - TudorAndrei
- (**compat**) support updated development tools - (c5f4457) - TudorAndrei
- (**core**) allow licenses for TLS dependencies - (d17d957) - TudorAndrei
- (**core**) allow LLVM exception in license gate - (df8dd29) - TudorAndrei
- (**gui**) honor Spotify watchlist audio source - (b63951f) - TudorAndrei
- (**gui**) show supported duplicate choices - (d7f5574) - TudorAndrei
- (**gui**) show native import events and match candidates - (dcfa899) - TudorAndrei
- (**gui**) avoid reading main view during config render - (8a4cf82) - TudorAndrei
- (**gui**) explain skipped Soulseek selection - (dfd046c) - TudorAndrei
- (**gui**) run GPUI app through cargo in mise task - (9e1d4dc) - TudorAndrei
- (**gui**) keep service reads off the command loop - (00e9d02) - TudorAndrei
- (**gui**) launch the current debug build from mise - (3ef591b) - TudorAndrei
- (**gui**) load saved sources on Spotify page - (9413f09) - TudorAndrei
- (**gui**) load thumbnails for current watchlist page - (534e193) - TudorAndrei
- (**gui**) keep watchlist context and show item details - (abbbebe) - TudorAndrei
- (**gui**) build native app with macOS Bash - (254afec) - TudorAndrei
- (**gui**) grow the watchlist card so its buttons don't force scrolling - (467eb6a) - TudorAndrei
- (**gui**) pin the Quit button to a fixed-width column - (519614c) - TudorAndrei
- (**gui**) make top nav a real tab bar instead of separate windows - (39f3569) - TudorAndrei
- (**gui**) show safe Beets album choices - (53ada6c) - TudorAndrei
- (**gui**) show Beets choices in pipeline - (dc0de95) - TudorAndrei
- (**gui**) set icon for direct launches - (a72bdbc) - TudorAndrei
- (**gui**) show watchlist refresh progress - (6b23090) - TudorAndrei
- (**gui**) limit watchlist textures to the current page - (f7e86c2) - TudorAndrei
- (**hooks**) scan unpushed commits before push - (cb7f0c7) - TudorAndrei
- (**hooks**) scan staged changes for secrets - (37092c3) - TudorAndrei
- (**import**) use audio duration for release matching - (826c679) - TudorAndrei
- (**import**) replace selected duplicate files safely - (6c977a0) - TudorAndrei
- (**import**) preserve move results and explicit singleton paths - (29a1c36) - TudorAndrei
- (**import**) match beets as-is album fields - (64f1fdc) - TudorAndrei
- (**import**) accept relative destination paths - (2b74ef1) - TudorAndrei
- (**library**) create database on first native import - (e04f547) - TudorAndrei
- (**library**) parse Unicode query terms safely - (ed90741) - TudorAndrei
- (**library**) match beets artist sort order - (a6e65d9) - TudorAndrei
- (**match**) name beets string replacement table - (45612b7) - TudorAndrei
- (**metadata**) normalize album title noise - (42dba46) - TudorAndrei
- (**native-gui**) preserve Spotify state and reconcile safely - (662be9e) - TudorAndrei
- (**native-gui**) handle malformed input and watchlist card data - (a48ae52) - TudorAndrei
- (**rust**) exclude GPUI app from crate workspace - (21d7654) - TudorAndrei
- (**soulseek**) show progress during check-library's Soulseek searches - (370ebd6) - TudorAndrei
- (**watchlist**) request JPEG thumbnails from YouTube - (601a44d) - TudorAndrei
- (**watchlist**) prevent action callback crash - (e95062c) - TudorAndrei
- (**watchlist**) load missing thumbnails on demand - (dc446d5) - TudorAndrei
- (**watchlist**) repair skipped beets items - (db37e93) - TudorAndrei
- (**workflow**) repair legacy split metadata - (893e390) - TudorAndrei
- (**workflow**) preserve album metadata for Beets - (cc177b2) - TudorAndrei
- (**workflow**) reject skipped beets imports - (5e434ca) - TudorAndrei
#### Documentation
- (**cli**) clarify default import behavior - (c08cc83) - TudorAndrei
- (**gui**) describe cargo run development task - (01a4227) - TudorAndrei
- (**gui**) describe native desktop build and service - (20aff75) - TudorAndrei
- (**homebrew**) describe GPUI source build - (6fd0a75) - TudorAndrei
- (**install**) select the matching native wheel - (a032938) - TudorAndrei
- (**plan**) correct Phase 2 design against the real soulseek-rs-lib API - (84099ff) - TudorAndrei
- (**plan**) design embedded Seakarr integration - (8a7ad9e) - TudorAndrei
- (**rust-port**) record final verification - (2de9bdb) - TudorAndrei
- (**rust-port**) record native migration checks - (1adb5f3) - TudorAndrei
- (**rust-port**) record completed native phases - (606ea29) - TudorAndrei
- (**rust-port**) track plan and completed phases - (2640695) - TudorAndrei
- (**seakarr**) explain direct acquisition and quality checks - (d7ea9ea) - TudorAndrei
- (**watchlist**) explain playlist viewer and item actions - (2f7eb49) - TudorAndrei
- describe the check and check-release tasks - (ace3709) - TudorAndrei
- remove completed migration plans - (d644367) - TudorAndrei
#### Tests
- (**gui**) cover native workflow option mapping - (d2c9c15) - TudorAndrei
- (**match**) cover tracks without length - (932ef1a) - TudorAndrei
- (**native-gui**) cover decisions watchlist and Spotify protocol - (cb0929a) - TudorAndrei
#### Build system
- (**gui**) keep DearPyGui out of runtime dependencies - (021dbb7) - TudorAndrei
- (**gui**) reduce GPUI release binary size - (a4007c6) - TudorAndrei
- (**gui**) verify GPUI sources in source archive - (27ab494) - TudorAndrei
- (**gui**) package native app with release wheels - (3ec6263) - TudorAndrei
- (**release**) package the Rust CLI and desktop app - (2ec85a8) - TudorAndrei
- (**release**) verify installed GPUI wheels - (6c5175d) - TudorAndrei
- (**release**) package the embedded Seakarr bridge - (eb63bd5) - TudorAndrei
- (**rust**) pin toolchain and required targets - (9bb9d7e) - TudorAndrei
#### Continuous Integration
- (**release**) pause Python publishing and check commits - (76c54a9) - TudorAndrei
- cache Rust builds and skip repeated release checks - (85da444) - TudorAndrei
#### Refactoring
- (**gui**) move saved config into a tab - (aad0998) - TudorAndrei
- (**gui**) remove retired DearPyGui implementation - (dbbd307) - TudorAndrei
- (**import**) remove obsolete beets adapter paths - (105b281) - TudorAndrei
- <span style="background-color: #d73a49; color: white; padding: 2px 6px; border-radius: 3px; font-weight: bold; font-size: 0.85em;">BREAKING</span>(**import**) remove legacy beets implementation - (94bc28e) - TudorAndrei
- <span style="background-color: #d73a49; color: white; padding: 2px 6px; border-radius: 3px; font-weight: bold; font-size: 0.85em;">BREAKING</span>(**import**) use native library and remove runtime dependencies - (f54a48f) - TudorAndrei
- (**match**) use published lsap solver - (30b5044) - TudorAndrei
- (**native**) split Soulseek bridge into muzik-soulseek and muzik-py - (696bba8) - TudorAndrei
- (**rust**) separate CLI app and share download inventory - (36a5908) - TudorAndrei
- (**soulseek**) replace slskd with embedded Seakarr - (dc571f6) - TudorAndrei
- (**workspace**) use top-level Rust app and crate folders - (8a8e6ce) - TudorAndrei
#### Miscellaneous Chores
- (**deps**) update Python dependencies - (8d760f6) - TudorAndrei
- (**gui**) update gpui-kit to 0.7.0 - (1ff0f6c) - TudorAndrei
- (**license**) license Rust desktop app under GPL-3.0-only - (9b9acc6) - TudorAndrei
- (**rust**) lock native import dependencies - (70944c6) - TudorAndrei
- (**rust**) lock native crate dependencies - (57787a8) - TudorAndrei
- (**todo**) record final verification and review status - (c59b91a) - TudorAndrei
- (**todo**) check off phase 8 documentation work - (6ea7491) - TudorAndrei
- (**todo**) check off phase 7 packaging work - (27c2a33) - TudorAndrei
- (**todo**) mark Phase 6 quality/watchlist items done, flag live transfer progress as not implemented - (eaabe7b) - TudorAndrei
- (**todo**) mark Phase 5 YouTube-first quality items done - (c7fec89) - TudorAndrei
- (**todo**) mark Phase 4 structured Spotify acquisition items done - (8a8c8b8) - TudorAndrei
- (**todo**) mark Phase 3 slskd-replacement items done - (e966b41) - TudorAndrei
- (**todo**) mark Phase 1 quality-decision items done - (ebebd78) - TudorAndrei
#### Style
- (**gui**) use GPUI Kit theme colors - (11cfa94) - TudorAndrei
- (**gui**) format watchlist card controls - (c7adf4a) - TudorAndrei
- (**library**) format transaction test - (2ce2b82) - TudorAndrei

- - -

Changelog generated by [cocogitto](https://github.com/cocogitto/cocogitto).