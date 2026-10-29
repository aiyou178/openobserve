# 支持 OIDC 的 OpenObserve OSS

[English](README.md) | [简体中文](README.zh-CN.md)

这是 [OpenObserve](https://github.com/openobserve/openobserve) 的独立 fork。
感谢上游作者和贡献者开发并开源日志、指标、链路追踪与仪表盘等可观测能力。
本 fork 为 OSS 版本增加**通用 OpenID Connect（OIDC）单点登录支持**，
不是 OpenObserve 官方发行版，也不启用私有 Enterprise 代码。

**本 fork 版本：** [`v1.1.2.4`](https://github.com/aiyou178/openobserve/tree/v1.1.2.4)

**上游基线：** [`v1.1.2` / `d16d693`](https://github.com/openobserve/openobserve/tree/d16d693ba99bc20621a58b948ecbcad7ec7fa882)

**开发分支：** [`dev`](https://github.com/aiyou178/openobserve/tree/dev)

所有版本在 `dev` 的历史上通过 tag 标记，过往版本 tag 保留原始提交。

平台本身的使用方法请参考 [OpenObserve 上游文档](https://openobserve.ai/docs/)。

## 本 fork 增加了什么

- 可选的标准 OIDC 浏览器 SSO，默认关闭。
- Authorization Code 流程，使用 PKCE S256、state、nonce、`client_secret_basic`，
  校验 RS256/JWKS，并核对 ID Token 与 UserInfo 的 subject 一致。
- 首次登录自动创建用户，以持久化的 `(issuer, sub)` 识别账号，由配置指定默认加入的组织。
- 兼容通过显式绑定关联已有非 Root 本地用户。
- 独立、可撤销的 HttpOnly 会话、CSRF 防护和当前组织成员关系检查。
- OpenObserve 前端中的 SSO 登录、会话初始化、退出、过期处理和 Source 下载入口。
- 本地 Linux amd64/arm64 打包，包含内嵌前端、准确对应源码、许可证、构建信息及 SHA-256 校验值。

原生登录、机器 Basic/API/OTLP 凭据保持独立。SSO 不会自动创建组织，
不会按邮箱合并账号，也不增加 Enterprise RBAC。
获准用户在允许的组织内具有本 OSS 版本的 **Admin 级能力**，请在启用自动建号或添加绑定前明确批准。
Root 和服务账号不能使用 SSO 登录。

## 启用 OIDC

1. 在身份提供方注册 confidential client，使用 Code 流程、PKCE S256、
   `client_secret_basic`、query response mode 和 `openid profile`。
   精确注册回调地址，例如 `https://observe.example.com/auth/oidc/callback`。
2. 预先创建新用户应加入的组织，将准确的组织 ID 填入
   `auto_provision.default_organizations`；新用户在这些组织中具有 OSS Admin 权限。
3. 将 [oidc.example.json](contrib/aiyou/oidc.example.json) 复制到服务端私有文件，
   例如 `/run/secrets/oidc.json`。填写 issuer、client ID、公开地址、独立的
   client-secret 文件路径，以及 `auto_provision.default_organizations`。
   设置 `allow_oss_admin: true` 批准 OSS 权限，无需逐用户维护 bindings。
   不配置 `auto_provision` 时，继续使用可选的显式绑定模式。
4. 设置环境变量并启动：

   ```bash
   export ZO_OIDC_ENABLED=true
   export ZO_OIDC_CONFIG_FILE=/run/secrets/oidc.json
   ./openobserve
   ```

5. 打开 `/web/`，选择 **SSO**。Root 管理继续使用原生登录，数据写入使用独立机器凭据。

使用 HTTPS 和精确的 issuer、callback 地址。不要将配置或凭据放入仓库或前端资源。
路径前缀、会话时限、绑定规则、代理隐私及退出恢复，详见
[OIDC 运维指南](contrib/aiyou/oidc-integration.md)。

## 前端与构建

OpenObserve 自己的前端改动均已包含在本仓库的 `web/` 中。
打包脚本会构建 `web/dist/` 并嵌入二进制，其中包括 SSO 控件和 Source 下载；
部署时不需要另外挂载 OpenObserve 前端目录。
身份提供方负责提供自己的认证与授权页面。

首次登录会在同一数据库事务中创建外部用户、默认组织成员关系及身份映射。
后续按 `(issuer, sub)` 找回同一账号，Provider 邮箱变化不会换号，也不会恢复已移除的成员关系。
已验证邮箱可用于初始化资料；缺失或未验证邮箱时使用稳定的 `@oidc.invalid` 标识。
邮箱与已有账号冲突时拒绝建号，不自动合并；不会发放可用的本地密码或机器 token。

在干净、已提交的 checkout 中，使用本地 Docker Linux engine 构建：

```bash
# Linux amd64
bash contrib/aiyou/build-binaries.sh amd64

# Linux arm64，或两个架构
bash contrib/aiyou/build-binaries.sh arm64
bash contrib/aiyou/build-binaries.sh all

# 本地运行镜像，默认使用 engine 的原生架构
bash contrib/aiyou/build-oidc-image.sh
```

二进制包默认输出到 `bin/oidc/`，包含 `BUILD-INFO.txt`、许可证、文档、准确源码和
`.sha256` 文件。fork 版本来自 `contrib/aiyou/VERSION`，上游 Cargo package 版本保持不变。
这些是 Linux GNU 二进制，不是静态 musl、macOS 或 Windows 构建。

构建使用仓库固定的 Rust nightly；该版本不可用时会停止，不会自动替换。
显式选择其他日期的工具链会记录在构建信息中，仍需验证兼容性。
构建前检查宿主机和 Docker 的磁盘、内存：二进制打包默认要求 40 GiB 可用空间、
单 Cargo 任务；镜像打包使用文档中说明的缓存余量阈值。
脚本不会自动发布或部署。依赖、可选参数、源码溯源及运行库要求，参见
[构建说明](contrib/aiyou/oidc-integration.md#source-and-local-container-build)。

## 上游致谢与许可证

原始平台由 [OpenObserve](https://github.com/openobserve/openobserve) 提供。
感谢上游作者和贡献者，本 fork 保留上游版权声明及
[许可证](https://github.com/openobserve/openobserve/blob/d16d693ba99bc20621a58b948ecbcad7ec7fa882/LICENSE)。
本 fork 按 [GNU Affero General Public License 第 3 版或更新版本](LICENSE) 分发。
OpenObserve 名称与商标归各自权利人所有，本 fork 不表示获得其背书。

打包产物通过登录页和登录后 Help 菜单的 Source 入口，提供该版本准确的对应源码和
构建说明。分发修改版或通过网络向用户提供服务时，请保留对应源码归档和许可证声明；
浮动分支链接不能代替已分发二进制的准确源码。
