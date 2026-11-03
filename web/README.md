# Web 账号管理

管理界面使用 Vue 3、TypeScript 与 Vite；页面为中文，支持桌面和移动布局。包括本地/公司 OIDC 登录、本人身份与会话、本人本地改密，以及管理员的用户维护、密码重置和目标会话管理；M3增加管理员审计筛选与游标分页；设备扩展增加“我的设备”和管理员设备归属维护。公司账号的密码和角色由 IdP 管理；最后一个可用本地管理员的保护由后端执行。

## 本地开发

要求 Node.js 20.19+ 或 22.12+（推荐 22 LTS），使用项目锁定依赖：

```sh
cd web
npm ci
npm run dev
```

访问 `http://127.0.0.1:5173/admin/`。Vite 默认仅绑定 `127.0.0.1`，固定使用 5173 端口，端口占用时会退出。开发代理将 `/admin/api` 和 `/api/oidc` 转发至 `http://127.0.0.1:21114`，保留浏览器请求 Origin。后端 **`RD_WEB_ORIGIN` 必须设为浏览器实际访问的 Vite origin**，本例为 `http://127.0.0.1:5173`。

浏览器始终调用同源相对路径，不能把浏览器 Origin 改写成 API 服务地址来绕过校验。OIDC 开发时，浏览器访问地址与 OIDC 回调地址必须使用相同 hostname，本例均使用 `127.0.0.1`；端口可以不同。若从 `localhost:5173` 登录，而回调到 `127.0.0.1:21114`，临时绑定 Cookie 的主机不同，会导致登录失败。OIDC 回调地址、临时 Cookie 与后端对外地址也应按部署环境配置；使用其他 hostname 时需同步调整浏览器地址、Vite 绑定、`RD_WEB_ORIGIN` 与 OIDC 回调配置。

## 验证与生产构建

```sh
npm test
npm run typecheck
npm run build
```

`npm test` 使用 Node 内置测试工具和 TypeScript 类型擦除功能，因此测试命令要求 Node 22.12+；生产构建仍遵循上述 engines 范围。`build` 先运行 `vue-tsc --noEmit`，再生成 `web/dist/`，资源 base 为 `/admin/`。后端 Axum 从该目录提供管理界面；启动生产服务前先执行 `npm ci`、`npm run build`。`npm run preview` 只用于预览静态产物，不包含开发代理。

如机器上的 npm 默认缓存不可写，可使用 `npm --cache /private/tmp/rustdesk-web-npm-cache ci`，无需修改全局缓存权限。

## 会话与敏感信息

登录使用 HttpOnly Cookie，所有 fetch 设置 `credentials: 'same-origin'`；写请求携带内存中的 `X-CSRF-Token`，请求 Origin 由浏览器设置。身份与 Token 不写入 localStorage、sessionStorage 或日志。提交密码后立即清空对应表单，退出或会话失效时清除内存状态。

401 会清除身份并返回登录页；403 显示权限不足。写请求不自动重试，停用、角色切换、密码重置及撤销会话需要本次操作确认。接口失败显示真实错误，成功后的列表从后端重新加载。普通用户不会渲染管理入口。管理员可查询账号审计，普通成员无此入口且后端403；页面不包含团队通讯录。审计加载器在退出、离开页面、筛选变更或401/403时清空数据，过时请求不能回填。

管理员重置密码的长度校验、两次输入不一致和接口失败提示显示在重置表单内；成功提示也显示在同一区域。编辑密码或关闭账号详情会清除该表单的旧反馈。重置本人密码成功或会话失效导致返回登录页时，登录提示仍显示在页面级区域。

依赖版本来自 npm registry；Vite 代理与 base 参考 [Vite 官方配置](https://vite.dev/config/server-options)，类型检查参考 [Vue TypeScript 文档](https://vuejs.org/guide/typescript/overview)。

设备页成员只请求本人设备；管理员可明确分配或解除归属。显示最近登录时间，不当作在线状态。退出、角色失效或页面销毁后清除设备及用户列表，过期响应不会回填；归属修改成功后重新读取服务端数据。归属变更会撤销绑定该设备的客户端账号会话，不控制远控连接。
