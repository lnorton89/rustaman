# Changelog

All notable changes to Rustaman, newest first.

**This file is generated.** It is rebuilt from the git history by
`cargo run --example changelog`, so released sections are rewritten in
place and edits to them do not survive. See `CONTRIBUTING.md`.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Nothing yet — the latest release is the tip of `main`.

## [0.1.0] - 2026-08-25

### Added

- **gui:** marks on the column headings that name a resource ([`392de38`](https://github.com/lnorton89/rustaman/commit/392de381762a1208bf5de86d4f6ceb81157fd19a))
- **dev:** let the harness render at a real display scale ([`ae7128d`](https://github.com/lnorton89/rustaman/commit/ae7128d002a681c925dad028faa87fba4e7deee3))
- **gui:** every process row carries a mark, not just Windows' own ([`ae70fdd`](https://github.com/lnorton89/rustaman/commit/ae70fdda3e0edc302a0382a98f98596ea3d8c34c))
- **win:** name the GPU from the registry, and report its capacity ([`8edabbd`](https://github.com/lnorton89/rustaman/commit/8edabbd306948d3fa64bac62208345ecb0078ff1))
- **gui:** give Windows' own processes a mark, and every row one left edge ([`bb2a823`](https://github.com/lnorton89/rustaman/commit/bb2a8238734ce6b660fa67694e3e22e3c3df2565))
- **win:** report Windows 11 efficiency mode and hybrid core topology ([`40e65ef`](https://github.com/lnorton89/rustaman/commit/40e65efac8459e2f7be1c427cff31c9c271e4c19))
- **gui:** round the window and colour its border on Windows 11 ([`729d84e`](https://github.com/lnorton89/rustaman/commit/729d84ea6fe18c2a2f9f791fc78130aa3a97125a))
- expand system views and harden Windows boundaries ([`d5706bc`](https://github.com/lnorton89/rustaman/commit/d5706bc3e4d5b6ee9ee120813d06ebd0c246eaf9))
- **gui:** graph the disk and the network by direction ([`509a0a6`](https://github.com/lnorton89/rustaman/commit/509a0a6e51f370b5444980ede11df1ef9127459d))
- **gui:** draw independent series on one axis, and the rings to feed them ([`4aa40dd`](https://github.com/lnorton89/rustaman/commit/4aa40dd9a355376b3614554dc976331416d63913))
- **gui:** a Memory view — a treemap of what is holding the machine's RAM ([`d658b09`](https://github.com/lnorton89/rustaman/commit/d658b09fde3eaa30946b6474bef7c0b44efd6c7d))
- **gui:** make the theme picker a gallery, and fix the grid it sits in ([`5054d0d`](https://github.com/lnorton89/rustaman/commit/5054d0d00467fdb4465c118f8346cb0cef7ff7ef))
- **dev:** render any view of the app to a PNG, without a window ([`5b097fc`](https://github.com/lnorton89/rustaman/commit/5b097fc13d58b32b8941d30265cee2d816020d47))
- **gui:** fade and rise the confirmation modal into place ([`16630c6`](https://github.com/lnorton89/rustaman/commit/16630c691ef2df1c12e633e08e751ead842ed681))
- **gui:** make the Services and Startup table headers actually sort ([`8c13b30`](https://github.com/lnorton89/rustaman/commit/8c13b30e276a52570a845695d9e74200e88d3369))
- **gui:** views fade in, and the numbers settle rather than flicker ([`aaf5a12`](https://github.com/lnorton89/rustaman/commit/aaf5a124ec1a87cba7b983af500b6eb1d4eee52a))
- **gui:** drag the column headings to reorder them ([`c1a8134`](https://github.com/lnorton89/rustaman/commit/c1a81342d671f2808aea6d00af4518a1b3870553))
- **gui:** the window — chrome, theming, graphs, and every view ([`486ba1c`](https://github.com/lnorton89/rustaman/commit/486ba1ceff31b9acf53990c5eea8e574397c6cbc))
- **engine:** sampler thread and the rate arithmetic behind it ([`a667ce4`](https://github.com/lnorton89/rustaman/commit/a667ce44f6f14af7418020b2df2ebc71f20267db))
- **win:** Win32 layer with safe leaf wrappers over ntdll and Win32 ([`cde0e47`](https://github.com/lnorton89/rustaman/commit/cde0e47c714533465b2f7cc0674680004b640a12))
- portable core — model, theming, formatting, config ([`0b0f3ca`](https://github.com/lnorton89/rustaman/commit/0b0f3cae7f5faa3cb16813af5cfe4fa1590b44b7))

### Fixed

- **gui:** tooltips at the pointer, not at the left edge of the row ([`736b5a9`](https://github.com/lnorton89/rustaman/commit/736b5a904cba8a97978654ce2f7b790018c4d19f))
- **gui:** name the GPU engines the counters leave unnamed ([`9aad6db`](https://github.com/lnorton89/rustaman/commit/9aad6dbb47f5a8d25b2126280f69046c88f17108))
- **gui:** the last three panel hairlines, and a test that finds them ([`a197d87`](https://github.com/lnorton89/rustaman/commit/a197d87456779cf9ee35ea272accaa79b802c672))
- **gui:** clearance before the scrollbar in the System and Settings views ([`ebd3445`](https://github.com/lnorton89/rustaman/commit/ebd3445408ada530ec58b07c785934df405e23d1))
- **gui:** stop every Performance panel showing a scrollbar it does not need ([`adb1842`](https://github.com/lnorton89/rustaman/commit/adb1842b68897d8efb0c93c5ef8dd4701c3b4e53))
- **icon:** the Windows mark is solid, because the real one is ([`8efb2dc`](https://github.com/lnorton89/rustaman/commit/8efb2dca06610ecfdf9f3b15406aa0b344c835fd))
- **gui:** keep the Details rows out of the scrollbar's lane ([`2e3f9b1`](https://github.com/lnorton89/rustaman/commit/2e3f9b1281c8980801fe8fe2ccf007cb6c7cc19c))
- **gui:** head the GPU panel with the adapter, and name the binary properly ([`87f476d`](https://github.com/lnorton89/rustaman/commit/87f476d9ec333997ea92706cadd231d139ab154e))
- **win:** GPU engines in numeric order, and the capacity older drivers wrote ([`771dc2c`](https://github.com/lnorton89/rustaman/commit/771dc2c8ac458581ae26ce35b961e63556fb6e23))
- **gui:** the Windows 11 border follows the theme instead of the startup one ([`0bae77d`](https://github.com/lnorton89/rustaman/commit/0bae77daf177c05c6b8f539f147ffe516c42571b))
- **gui:** stat captions that fit, so the values line up ([`a2d374c`](https://github.com/lnorton89/rustaman/commit/a2d374c46c77af372d87a8b0419d8d7bd2492e63))
- **gui:** open the window on D3D12 rather than whatever wgpu picks (#2) ([`5831ef0`](https://github.com/lnorton89/rustaman/commit/5831ef0dd0442fcc28219933cf206ca5a451fc64))
- **gui:** let the treemap fit its pane instead of scrolling ([`9ecd3bc`](https://github.com/lnorton89/rustaman/commit/9ecd3bc8e6c28be5ddb4a9d4de6019fec4b660b2))
- **gui:** scroll the service and startup lists instead of cutting a column ([`fe71e08`](https://github.com/lnorton89/rustaman/commit/fe71e081b14ebb63bee0a40b65fe5c12fd479c1c))
- **gui:** drop the Startup note when the toolbar has no room for it ([`fd53d5e`](https://github.com/lnorton89/rustaman/commit/fd53d5e71718fefae1534295b3e9edc60de10860))
- **win:** let the icon reader's GDI handles own themselves ([`d8d8a63`](https://github.com/lnorton89/rustaman/commit/d8d8a63c922f3a579c24c764730e52de9638f73e))
- **gui:** scroll the process table sideways instead of clipping columns ([`d8f6e8b`](https://github.com/lnorton89/rustaman/commit/d8f6e8bc44442ebb17dfcabc54a63d98001d6871))
- **gui:** stop a half-row painting through the bottom of every table ([`a79b76b`](https://github.com/lnorton89/rustaman/commit/a79b76bd25f67b23ed6272eed1f965a1ae12dd02))
- **gui:** the four points of grey down a hovered row's leading edge ([`fa8341f`](https://github.com/lnorton89/rustaman/commit/fa8341fa8bad1344a886ce12b46d3782a3353b68))
- **gui:** scroll the Details table sideways rather than dropping columns ([`2f1d8bd`](https://github.com/lnorton89/rustaman/commit/2f1d8bddf9bd357f5e3578130f73ab749a654410))
- **gui:** a Performance panel that takes the pane it is given ([`ec8efcc`](https://github.com/lnorton89/rustaman/commit/ec8efcc8220fa0d80698a0efbaa02e71db7be1d7))
- **dev:** take the default window size from the app, not from a test fixture ([`8f8a08e`](https://github.com/lnorton89/rustaman/commit/8f8a08ea928b5a8b1d207cc8f4c21ef1b8ac32a2))
- **gui:** a row highlight that covers the row, in every table ([`d9e2b61`](https://github.com/lnorton89/rustaman/commit/d9e2b6160430cddc3acdb5d7198c693b96edcee5))
- **win:** stop a not-ready drive parking the sampler on a modal dialog ([`19abbd6`](https://github.com/lnorton89/rustaman/commit/19abbd6038bfa2ad3265b2a6e0fe64f0d84b22c3))
- **engine:** bound the wait for the sampler so closing cannot hang ([`23399c8`](https://github.com/lnorton89/rustaman/commit/23399c8395ed329ab86dd10c0a986c6cceaaa7b3))
- **gui:** the process table's hover, its sorting, and its column widths ([`161181b`](https://github.com/lnorton89/rustaman/commit/161181b397f9006055b4caa94d04a1a49858f717))
- **ci:** allow the Ubuntu Font Licence under the name SPDX gave it ([`80a50b7`](https://github.com/lnorton89/rustaman/commit/80a50b7787f9804ff8e4e195bfecaf690d6068e1))
- **gui:** stop the CPU panel reserving core-grid height from a second copy of the formula ([`21410e9`](https://github.com/lnorton89/rustaman/commit/21410e92503c03ac4b332b7600b156369bfdbeeb))
- **gui:** make a table row look like one row ([`a4bddc4`](https://github.com/lnorton89/rustaman/commit/a4bddc49b44febe39a0a4f9616b5df44c66fc014))
- **net:** list every adapter, and stop counting filter modules as adapters ([`f43ec20`](https://github.com/lnorton89/rustaman/commit/f43ec20fc1b82b65c6f4e4b009aa9891e66b051c))
- **gui:** stop the picker/detail spacer from drawing its own separator line ([`da72828`](https://github.com/lnorton89/rustaman/commit/da72828ed2e8673b9354a78f35a3d13a441aadde))
- **gui:** the picker/detail seam had no gap at all, not a narrow one ([`b5a0b21`](https://github.com/lnorton89/rustaman/commit/b5a0b21c4621f337f1c18fff61cd20581c2588bc))
- **gui:** give the picker's own rows margin before the detail column ([`02fa31c`](https://github.com/lnorton89/rustaman/commit/02fa31c4025beba022021c58819ec23713e12bd9))
- **gui:** stop the CPU panel's own content overflowing the window edge ([`ed38958`](https://github.com/lnorton89/rustaman/commit/ed389585bbbc8110e3b88ed21e57ccf0aef0b5a0))
- **gui:** give the Performance detail column real trailing margin ([`8650df5`](https://github.com/lnorton89/rustaman/commit/8650df5401fdbda818976cff52979ed7d18f741f))
- **gui:** cache the Services and Startup views' filtered, sorted lists ([`2be0ada`](https://github.com/lnorton89/rustaman/commit/2be0ada0ba3e4249ff20c76a0cf91e812337da38))
- **gui:** move Services and Startup enumeration off the UI thread ([`c8a8ac2`](https://github.com/lnorton89/rustaman/commit/c8a8ac2b3df280028de7cb4c2cc941d260b191ba))
- **gui:** label the per-core CPU tiles with a hover readout ([`8613b41`](https://github.com/lnorton89/rustaman/commit/8613b412aaa3f710f26ad1eaa48184d394c6ef1a))
- **gui:** sort the disk and GPU device grids busiest-first too ([`28f9413`](https://github.com/lnorton89/rustaman/commit/28f94135b95bf56561d944673049d839295ebbf8))
- **win:** stop silently failing to read per-core and total CPU ([`3b13462`](https://github.com/lnorton89/rustaman/commit/3b1346291cb0d1010e76a3226ac70d2e62c41f3f))
- **gui:** derive the tree-row indent from the disclosure control it matches ([`c26542f`](https://github.com/lnorton89/rustaman/commit/c26542fd5b922c4b8ee0736df74097d71a09b54f))
- **gui:** teach the drag-state lint about the window's own chrome ([`cc7a2a6`](https://github.com/lnorton89/rustaman/commit/cc7a2a6542dc3342890728fa9a36ec962dc0040c))
- **gui:** sort the Network view busiest-first and collapse the idle adapters ([`36475ef`](https://github.com/lnorton89/rustaman/commit/36475efd32d2f64e035bdf7a474eee7492e6b4bc))
- **gui:** the row highlight, the table width, the heat tint, the card grid ([`110a27e`](https://github.com/lnorton89/rustaman/commit/110a27e1faf0e06c1b9f87ea0a327bc595731d58))
- **gui:** draw the icons, and put every animation on one clock ([`e08191a`](https://github.com/lnorton89/rustaman/commit/e08191a776f4812d54af0c530a70363bf9a0f1a5))

### Changed

- **gui:** give the treemap the window, and stop the core grid repeating itself ([`f35c4de`](https://github.com/lnorton89/rustaman/commit/f35c4de2ce9d3766f729ed4eae1a5e047812fdc9))

### Documentation

- record the SystemProcessorPerformanceInformation buffer quirk ([`13512e1`](https://github.com/lnorton89/rustaman/commit/13512e17ecae79a1459a13cb94e82e45c4dfda45))
- document BackgroundRead as the second off-UI-thread pattern ([`12b6c29`](https://github.com/lnorton89/rustaman/commit/12b6c29c4c5d07222a836789191f708cdce1c60d))
- the enforced-rules doc, the architecture notes, and CI ([`f4eb779`](https://github.com/lnorton89/rustaman/commit/f4eb7791be2bd07e881910919bbf70be90e6058e))

### Internal

- **deps:** bump toml from 0.9.12+spec-1.1.0 to 1.1.4+spec-1.1.0 ([`e8e514e`](https://github.com/lnorton89/rustaman/commit/e8e514e3abe0d777dcba75d399a97cfb153bc244))
- embed the icon and manifest, and generate the brand assets ([`80a5080`](https://github.com/lnorton89/rustaman/commit/80a5080c289338ebfb44982977f6e6b299f97d1f))

### Tests

- **gui:** run both view invariants at a real display scale ([`918c41b`](https://github.com/lnorton89/rustaman/commit/918c41b105a1337fab4ca3c8b539af8a7145fbff))
- **gui:** fail the build when the icon list misses a variant ([`05f7a20`](https://github.com/lnorton89/rustaman/commit/05f7a20017fb3463aa592e7e478ce4e7bd9707e0))
- **gui:** catch a view that never stops repainting ([`f2e928a`](https://github.com/lnorton89/rustaman/commit/f2e928ad3ca630027dacdb0502c9c02012559527))
- **gui:** hold every view to the pane it was given, on every edge ([`d3d3918`](https://github.com/lnorton89/rustaman/commit/d3d39189bf3d0a5bb231354eb36c83b84f874e81))
- **gui:** hold all four tables to the rect they were given ([`29c9ed3`](https://github.com/lnorton89/rustaman/commit/29c9ed3be62028d96b8e333d13f627a9450ce4a2))
- **gui:** check the motion binding's NaN guard against a real Context ([`fc02707`](https://github.com/lnorton89/rustaman/commit/fc027073bcea00003ea8cd3c3864607d0a91d476))
- **gui:** pin the row fill that a comment claimed was already fixed ([`4fef576`](https://github.com/lnorton89/rustaman/commit/4fef576c36df1263c73dab0a08b6423eb42065b9))
- **gui:** stop the core-grid squareness test from grading its own copy ([`b65a599`](https://github.com/lnorton89/rustaman/commit/b65a59901af975c8e19f71f7944b63d4ed03d8fe))
- **gui:** extract card_grid's column math so it can be checked directly ([`0d692ef`](https://github.com/lnorton89/rustaman/commit/0d692efcab7d0d645517f82b85269ad9f150b83f))
- **gui:** check the Performance picker/detail seam at the window's own minimum size ([`d42c035`](https://github.com/lnorton89/rustaman/commit/d42c035b4c8f1ba564d3b25b3b0138034b6c3026))
- **gui:** check all five Performance panels against the window edge ([`7c75f99`](https://github.com/lnorton89/rustaman/commit/7c75f9996f276de9296aa2a879af3fb7b3366533))
- **gui:** clear the discarded FullOutput's texture delta ([`5469f4c`](https://github.com/lnorton89/rustaman/commit/5469f4cb8434dddd74426936170ab08664aed796))

[Unreleased]: https://github.com/lnorton89/rustaman/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/lnorton89/rustaman/releases/tag/v0.1.0
