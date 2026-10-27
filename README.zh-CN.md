# 支持 OIDC 的 OpenObserve OSS

[English](README.md) | [简体中文](README.zh-CN.md)

这是 [OpenObserve](https://github.com/openobserve/openobserve) 的独立公开 fork。
OpenObserve 提供日志、指标、链路追踪和仪表盘等可观测能力，感谢其作者及贡献者
开发并开源原始平台。本 fork 基于上游 **v1.1.2**
（`d16d693ba99bc20621a58b948ecbcad7ec7fa882`），fork 发布版本为 **v1.1.2.1**。
本项目不是 OpenObserve 官方发行版，也不是 Enterprise 版本。

平台本身的使用方法请参考 [上游文档](https://openobserve.ai/docs/) 和
[v1.1.2 源码](https://github.com/openobserve/openobserve/tree/d16d693ba99bc20621a58b948ecbcad7ec7fa882)。

## 本 fork 增加了什么

- 对接通用 OpenID Connect 身份提供方的浏览器单点登录，默认关闭。
- Authorization Code 流程，使用 PKCE S256、state、nonce；服务端通过
  `client_secret_basic` 交换令牌，并校验 RS256/JWKS 和 UserInfo 的 subject。
- 显式将 `(issuer, sub)` 绑定到已有非 Root 本地用户和组织白名单；独立、可撤销的
  HttpOnly 会话，以及 CSRF 防护。
- 在打包产物中提供 Source 入口，下载该构建对应的准确源码。
- 使用本地 Docker 构建 Linux amd64、arm64 二进制，包含前端、许可证、对应源码、
  构建信息和 SHA-256 校验文件。

原生登录、机器使用的 Basic/API/OTLP 认证保持独立。SSO **不会**增加 Enterprise RBAC、
Viewer/只读权限控制或自动创建用户。获准的 SSO 用户在允许的组织内具有本 OSS 版本的
**Admin 级能力**，请在增加绑定前明确批准该权限。Root 用户和服务账号不能通过此 SSO
入口登录。

## 前端归属与打包

OpenObserve 自己的前端源码在本仓库的 `web/`：SSO 按钮、回调及会话初始化、退出和
Source 下载入口均已包含。二进制与镜像构建脚本会构建 `web/dist/`，并将其嵌入
OpenObserve 二进制；部署该二进制时不需要再挂载一份 OpenObserve 前端目录。

Dispatcher 的中央登录、同意授权页是独立的身份提供方应用，源码属于
`dispatcher_admin` 的 `apps/sso/` 及其共享 `packages/features/auth-shell/` 等依赖。
机器上的 `sso-dist/`（包含 `index.html`、`config.json` 和 `assets/`）是这套应用的
构建产物，并非 OpenObserve 遗漏的前端源码。它应按 Dispatcher 前端的发布流程
构建、发布，并由 Provider 配置的同源地址和路径提供服务，不复制进这个通用
OpenObserve RP 或它的对应源码归档。其他 OIDC Provider 使用各自的登录页面。

本 fork 的发布代码位于独立分支
[`codex/oidc-v1.1.2.1`](https://github.com/aiyou178/openobserve/tree/codex/oidc-v1.1.2.1)，
对应 tag 为 [`v1.1.2.1`](https://github.com/aiyou178/openobserve/tree/v1.1.2.1)。
上游默认分支的实际名称是 `main`；该发布分支保留指定的 v1.1.2 基线，
不会随默认分支升级。

## 启用 OIDC

1. 运行本 fork 的二进制，准备一个已有的**非 Root 本地用户**，确保其在目标已有组织中
   具有 Admin 成员身份。通过经授权的元数据管理方式取得不可变的本地 `users.id`。
2. 在身份提供方注册 confidential client：

   | 配置项 | 值 |
   | --- | --- |
   | 授权流程 | Authorization Code |
   | PKCE | S256 |
   | Token endpoint 认证 | `client_secret_basic` |
   | Response mode | `query` |
   | Scopes | `openid profile` |
   | 精确回调地址 | `https://observe.example.com/auth/oidc/callback` |

   身份提供方必须提供 discovery、RS256 JWKS 和 UserInfo。各端点必须与配置的
   issuer 同源，并位于其路径前缀之下；不支持任意其他域名的端点或 HTTP 重定向。
3. 将 [oidc.example.json](contrib/aiyou/oidc.example.json) 复制到服务端私有只读挂载，
   例如 `/run/secrets/oidc.json`，填写 issuer、client ID、公开地址及密钥文件路径。
   Client secret 单独放在挂载文件中，不要提交到仓库或放进前端环境变量。
4. 在 `bindings` 中加入已批准的绑定：

   ```json
   {
     "issuer": "https://idp.example.com/oidc",
     "subject": "STABLE_OPAQUE_SUBJECT_FROM_YOUR_PROVIDER",
     "email": "existing-user@example.com",
     "user_id": "EXISTING_IMMUTABLE_LOCAL_USER_ID",
     "allow_oss_admin": true,
     "organizations": ["EXISTING_ORGANIZATION_ID"]
   }
   ```

   `email` 用于查找已有本地记录，**不会**根据身份提供方声明的邮箱自动关联账号。
   不可变的本地用户 ID 与外部 `(issuer, sub)` 必须同时匹配。空 `bindings` 拒绝所有身份。
5. 设置环境变量并启动：

   ```bash
   export ZO_OIDC_ENABLED=true
   export ZO_OIDC_CONFIG_FILE=/run/secrets/oidc.json
   ./openobserve
   ```

   issuer 和公开地址均须使用 HTTPS，不能以 `/` 结尾，也不能包含 query 或 fragment。
   若公开地址包含路径前缀，例如 `https://observe.example.com/observe`，同时设置
   `ZO_BASE_URI=/observe`，并精确注册
   `https://observe.example.com/observe/auth/oidc/callback`。
6. 打开公开地址下的 `/web/`，点击登录页的 **SSO**。Root 管理继续使用原生登录；
   数据写入继续使用独立机器凭据。不要将 OTLP/API 写入入口重定向到交互式登录。

配置文件和密钥挂载仅允许服务进程和授权运维人员读取，不要提交真实身份、凭据或日志。
会话有效期、代理脱敏、撤销、仅限本机开发配置及排障方法，参见
[OIDC 运维指南](contrib/aiyou/oidc-integration.md)。

## 本地构建二进制

在干净、已提交的 checkout 中运行
[build-binaries.sh](contrib/aiyou/build-binaries.sh)。脚本使用本地 Ubuntu 24.04
Linux 容器构建前端及 `release-prod` OSS Rust 二进制，不启用 Enterprise，
不使用 GitHub Actions。Docker 必须连接**本地** Linux engine，脚本拒绝远程
Docker context。建议至少给 Docker 分配 16 GiB 内存，并为每个架构预留至少
40 GiB 构建空间。在 ARM 主机上构建 amd64 会使用 Docker 的 Linux 模拟，
可能明显慢于原生构建。
amd64 的 Rust 构建步骤使用进程级 QEMU 客体地址偏移，避免 ARM 模拟时的分配器
地址冲突；不修改 Docker 全局设置或编译目标。

```bash
# 默认构建 Linux amd64
bash contrib/aiyou/build-binaries.sh

# 构建 Linux arm64，或依次构建两个架构
bash contrib/aiyou/build-binaries.sh arm64
bash contrib/aiyou/build-binaries.sh all
```

脚本归档准确的已提交 revision，默认将产物写入 `bin/oidc/`。不会推送镜像或提交、
创建 release、部署。构建前须先审查并提交未提交或未跟踪的源码修改，
不要为了通过干净检查而提交密钥。

| 可选环境变量 | 含义 |
| --- | --- |
| `OIDC_BUILD_OUTPUT_DIR` | 产物目录，默认为仓库下的 `bin/oidc/` |
| `OIDC_BUILD_JOBS` | Cargo 并行构建任务数，默认 `1` |
| `OIDC_RUST_TOOLCHAIN` | 显式指定其他日期的 nightly，例如 `nightly-YYYY-MM-DD` |
| `OIDC_MIN_FREE_GB` | 最低可用空间安全阈值，单位 GiB，默认 `40` |

降低空间阈值或增加并行任务前，请先检查 Docker 的可用磁盘与内存。
Docker Desktop 虚拟机磁盘也须有足够空间，仅宿主机空间充足并不代表满足要求。

二进制包名称：

```text
openobserve-linux-amd64-v1.1.2.1.tar.gz
openobserve-linux-arm64-v1.1.2.1.tar.gz
```

每个包都有 `.sha256` 文件，包内含 `openobserve`、`LICENSE`、中英文 README、
`contrib/aiyou/` 下的 OIDC 指南与示例、`BUILD-INFO.txt` 和准确的对应源码归档。二进制内嵌前端及 Source 下载入口。
解压前检查校验值，运行前查看 `BUILD-INFO.txt` 中的 revision、工具链及动态库依赖。
这些是 **Linux GNU 二进制**，不是静态 musl、macOS 或 Windows 包。
构建成功不等于已部署或已完成 OIDC 验收。

脚本使用仓库固定日期的 Rust nightly；若官方发行文件不可用，会报错停止，不会自动
切换工具链。显式指定其他日期的 nightly 时，构建信息会记录该覆盖值，仍须验证兼容性。

如需从干净、已提交的源码快照构建本地 Linux 运行镜像，可运行：

```bash
bash contrib/aiyou/build-oidc-image.sh
# 或显式指定一个架构：
bash contrib/aiyou/build-oidc-image.sh amd64
```

脚本默认使用本地 engine 的原生架构，与二进制打包复用 `release-prod` builder、
固定工具链和默认单任务，包含前端、许可证与准确的对应源码归档。只连接 Docker Desktop
的本地 `desktop-linux` Unix socket。只读探针确认相同架构和工具链已有 release 二进制及
依赖缓存时，原生构建的启动余量为 16 GiB；未确认目标产物的原生构建使用 20 GiB，
模拟构建保留 40 GiB。
缓存构建预算包括 6 GiB 重编译产物、4 GiB 前端及工具链层、1 GiB 导出及上下文和
5 GiB 保留空间；未确认目标产物时另加 4 GiB 依赖编译余量。这是余量策略，不是实测最低需求。
脚本检查宿主机和 Docker 文件系统的可用空间，
任一低于 5 GiB 保留空间时取消本次构建，不删除缓存，也不覆盖已有镜像标签。
`OIDC_MIN_FREE_GB` 只能提高探测出的阈值。镜像标签为 `openobserve-oidc:<完整 revision>-<架构>`。脚本验证镜像平台、
revision、版本标签和实际 `--version` 输出，不启动服务器、不联网运行、不推送、不部署。
细节见 [运维指南](contrib/aiyou/oidc-integration.md#source-and-local-container-build)。
上游 Cargo package 版本保持不变；fork 发布版本由 `contrib/aiyou/VERSION` 提供。

## 许可证与致谢

本公开 OSS fork 使用 [GNU Affero General Public License 第 3 版或更新版本](LICENSE)，
保留上游版权和许可证声明。新增 OIDC 接入在公开 OSS 代码中独立实现，
不启用或包含私有 Enterprise 模块。

分发修改版或通过网络向用户提供服务时，请保留相关声明，并提供醒目的入口，
让用户取得该版本准确的对应源码和构建说明。提供的打包流程内嵌对应源码，
并通过登录页及登录后的 Help 菜单提供下载；分发二进制时不要替换成浮动分支链接。

OpenObserve 名称和商标属于各自权利人，本 fork 不表示获得其背书。
对原平台的贡献请遵循 [上游贡献指南](CONTRIBUTING.md)。
