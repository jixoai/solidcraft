# FDE2E SolidCraft

面向人与 AI agent 的参数化 3D CAD 工具。**基于 [SolidCraft](https://github.com/tangency-labs/solidcraft)（MIT OR Apache-2.0）**，由 FDE2E 维护，沿用纯 Rust 几何引擎、草图约束求解器、特征时间线和浏览器界面。

```text
人类操作 ────────────▶ egui 界面 ────┐
浏览器 agent ────────▶ WebMCP ───────┼──▶ 同一命令注册表 / 设计状态 / 撤销历史
桌面与命令行 agent ─▶ CLI / MCP ────┘
浏览器耗时建模 ─────▶ Rust/Wasm Worker ─▶ 精确求值结果与显示缓存
```

对外品牌为 **FDE2E SolidCraft**；本 fork 不代表上游，也不使用上游的官网、社媒、赞助入口或发行渠道。

## 项目入口

- 源码与问题反馈：[jixoai/solidcraft](https://github.com/jixoai/solidcraft)、[Issues](https://github.com/jixoai/solidcraft/issues)。
- 产品需求：[PRD](docs/PRD.md)。开发与浏览器构建：[构建说明](docs/building.md)。
- Agent 接入：[WebMCP / 浏览器 API](docs/browser-api.md)、[MCP](docs/mcp.md)、[建模技能](skills/solidcraft-modeling/SKILL.md)。
- 产品能力：[能力说明](docs/capabilities.md)、[路线图](ROADMAP.md)。
- FDE2E 项目参考：[FDE2E Film](https://github.com/jixoai/fde2e-tools-film)。这是同品牌项目，不是本 CAD 工具的部署地址。

本分支尚未发布 FDE2E 安装包或确认公开 Web 部署地址；请从源码构建。不要将上游下载地址、Snap/Flathub/WinGet 条目或上游签名状态视为本 fork 的发行承诺。

## Fork 链与维护

- 上游锚点：`tangency-labs/solidcraft@942e1540db2f38e9df36b8b91730cb989d6261d9`；来源与品牌边界见 [FORK.md](FORK.md)。
- `main` 保持上游镜像，`feat/webmcp-worker` 保存 WebMCP、Worker、产品文档和 FDE2E 品牌改造。
- 上游 remote 为 `upstream`，默认推送到 `origin`；不会直接推送上游或 force-push `main`。
- 后续更新按 [fork 维护指南](docs/fork-maintenance.md)备份、同步、rebase 和验证；不自动向上游开 PR。

## 开发

```sh
cargo run --release -p solidcraft -- --sample
cargo run --release -p solidcraft-cli -- commands
cargo run --release -p solidcraft-cli -- run examples/bracket.json --out bracket.step
rustup target add wasm32-unknown-unknown
cd apps/solidcraft-web && trunk serve --public-url ./
```

浏览器仍使用既有 `window.solidcraft` API 与 `solidcraft_*` WebMCP 工具名。Worker 模式可通过 `agent_off_thread` 偏好切换；同一设计支持草图、实体操作、参数修改、撤销和导出。工具能力与限制以命令描述和 [浏览器 API 文档](docs/browser-api.md)为准。

验证入口：`cargo xtask ci`。当前整套 CI 的既有失败与已验证范围见 [fork 维护指南](docs/fork-maintenance.md)，不能把针对性验证等同于整套通过。

## 许可与署名

MIT OR Apache-2.0，任选其一。上游 [LICENSE-MIT](LICENSE-MIT)、[LICENSE-APACHE](LICENSE-APACHE)、[NOTICE](NOTICE)、[ATTRIBUTION.md](ATTRIBUTION.md) 和 [THIRD-PARTY-LICENSES.txt](THIRD-PARTY-LICENSES.txt) 保留；新增贡献沿用本项目许可，不移除原作者版权或第三方署名。

项目独立于商业 CAD 厂商；遵守 [clean-room 规则](AGENTS.md)。软件按原样提供，许可中的无担保条款及 [免责声明](DISCLAIMER.md)仍适用。
