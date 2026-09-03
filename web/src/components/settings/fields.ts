// ============================================================================
// 系统设置：键说明字典（P-Settings 自建）
// 依据 config.example.yaml 注释与 M8 契约 §4.9/§5。
// key 为「点分全键」如 gateway.display_currency；未收录键展示兜底为 key 本身。
// ============================================================================

export interface SettingField {
  label: string
  help: string
}

/** 点分全键 → 中文 label/help（help 含默认值与单位） */
export const SETTING_FIELDS: Record<string, SettingField> = {
  // —— server（启动类）——
  'server.listen': {
    label: '监听地址',
    help: '监听地址（host:port）。环境变量 NEXTAPI_LISTEN 覆盖。启动类，需重启。默认 0.0.0.0:8080',
  },
  'server.admin_jwt_secret': {
    label: 'JWT 签名密钥',
    help: '管理端 JWT 签名密钥，生产必填。敏感字段；环境变量 NEXTAPI_ADMIN_JWT_SECRET 覆盖。启动类，需重启。默认空（非 debug 模式拒绝启动）',
  },
  'server.debug': {
    label: '调试模式',
    help: '放宽启动检查，仅本地开发使用。启动类，需重启。默认 false',
  },

  // —— database（启动类）——
  'database.url': {
    label: '数据库连接串',
    help: 'Postgres 连接串。启动类，需重启；优先级 env DATABASE_URL > UI > YAML。默认 postgres://nextapi:nextapi@localhost:5432/nextapi',
  },

  // —— gateway 网关运行参数 ——
  'gateway.default_rate_limit_rpm': {
    label: '默认限流 RPM',
    help: '每个 Key 每分钟请求数上限。默认 600',
  },
  'gateway.default_timeout_secs': {
    label: '默认上游超时',
    help: '非流式请求上游超时（秒）。默认 300',
  },
  'gateway.stream_timeout_secs': {
    label: '流式上游超时',
    help: '流式请求上游超时（秒）。默认 600',
  },
  'gateway.billing_timezone': {
    label: '计费时区',
    help: '计费时区（分时段/窗口起点/日志日界）。默认 Asia/Shanghai',
  },
  'gateway.display_currency': {
    label: '展示币种',
    help: '混合统计默认展示币种（CNY|USD）。默认 CNY',
  },
  'gateway.display_precision': {
    label: '展示精度',
    help: '金额展示舍入小数位（DB 仍 NUMERIC(20,10)）。默认 6',
  },
  'gateway.fx_stale_max_minutes': {
    label: '汇率过期阈值',
    help: '最近成功汇率超过该分钟数视为过期不可用。默认 1440',
  },
  'gateway.log_async': {
    label: '异步写库',
    help: '异步写库；false=同步（调试用）。默认 true',
  },
  'gateway.log_queue_capacity': {
    label: '日志队列容量',
    help: '有界队列容量；溢出先写 WAL 再丢弃内存条目。默认 10000',
  },
  'gateway.log_wal_dir': {
    label: 'WAL 目录',
    help: 'WAL 落盘目录。默认 /data/wal',
  },
  'gateway.log_wal_file_max_mb': {
    label: 'WAL 单文件上限',
    help: 'WAL 单文件大小上限（MB）。默认 128',
  },
  'gateway.log_wal_archive_tail_mb': {
    label: 'WAL 归档尾部',
    help: '尾部 N MB 全部写入并重放完成即归档。默认 64',
  },
  'gateway.log_partition_days': {
    label: '日志分区天数',
    help: 'usage_logs 每分区覆盖天数（30 天分文件存储）。默认 30',
  },
  'gateway.log_detail_retention_days': {
    label: '明细保留天数',
    help: '明细不自动清理；仅作手动清理默认 before（0=永久）。默认 0',
  },
  'gateway.log_aggregate_retention_days': {
    label: '汇总保留天数',
    help: '汇总表保留建议值，不自动删除（0=永久）。默认 365',
  },
  'gateway.log_debug_ttl_minutes': {
    label: 'Debug 最长时长',
    help: '按 Key debug 模式最长开启时长（分钟）。默认 60',
  },
  'gateway.admin_login_rate_limit_per_min': {
    label: '登录限速',
    help: '登录接口防爆破限速（次/分钟）。默认 10',
  },
  'gateway.anthropic_default_max_tokens': {
    label: 'Anthropic 默认 max_tokens',
    help: '转 Anthropic 缺省 max_tokens（64k）。默认 65536',
  },
  'gateway.batch_insert_interval_ms': {
    label: '批量写库间隔',
    help: '批量写库批次间隔（毫秒）。默认 500',
  },
  'gateway.quota_check_cache_secs': {
    label: '用量缓存秒数',
    help: '用量上限聚合结果缓存秒数。默认 3',
  },
  'gateway.quota_exceed_action': {
    label: '超配额动作',
    help: 'block=429 硬阻断 | warn=仅日志告警。默认 block',
  },

  // —— fx_auto_fetch 汇率自动拉取 ——
  'fx_auto_fetch.enabled': {
    label: '启用自动汇率',
    help: '是否启用外部汇率自动拉取（source=auto）。默认 true',
  },
  'fx_auto_fetch.provider': {
    label: '汇率数据源',
    help: 'frankfurter | ecb | custom。默认 frankfurter',
  },
  'fx_auto_fetch.base': {
    label: '基准币种',
    help: '拉取汇率的基准货币。默认 USD',
  },
  'fx_auto_fetch.symbols': {
    label: '目标币种',
    help: '需要兑换的目标币种列表（回车添加）。默认 [CNY]',
  },
  'fx_auto_fetch.refresh_interval_minutes': {
    label: '刷新间隔',
    help: '自动拉取间隔（分钟）。默认 60',
  },
  'fx_auto_fetch.cache_ttl_minutes': {
    label: '缓存有效期',
    help: '汇率缓存 TTL（分钟）。默认 60',
  },
  'fx_auto_fetch.timeout_secs': {
    label: '请求超时',
    help: '拉取汇率超时（秒）。默认 10',
  },
  'fx_auto_fetch.use_proxy': {
    label: '使用代理',
    help: '外联代理矩阵：拉取汇率是否走代理。默认 false',
  },
  'fx_auto_fetch.proxy_id': {
    label: '代理 UUID',
    help: '指定代理 UUID，留空跟随默认（use_proxy=true 时）。默认空',
  },

  // —— price_import 价格表导入 ——
  'price_import.allow_url': {
    label: '允许 URL 导入',
    help: '是否允许从 URL 导入价格表（仅 https，SSRF 防护）。默认 true',
  },
  'price_import.max_size_mb': {
    label: '最大大小',
    help: '导入内容最大大小（MB）。默认 1',
  },
  'price_import.timeout_secs': {
    label: '请求超时',
    help: '导入拉取超时（秒）。默认 10',
  },
  'price_import.use_proxy': {
    label: '使用代理',
    help: '价格 URL 导入是否走代理。默认 false',
  },
  'price_import.proxy_id': {
    label: '代理 UUID',
    help: '指定代理 UUID，留空跟随默认（use_proxy=true 时）。默认空',
  },

  // —— media_download 媒体下载 ——
  'media_download.enabled': {
    label: '启用媒体下载',
    help: '媒体 URL 下载转内联（默认关闭，SSRF 防护）。默认 false',
  },
  'media_download.max_size_mb': {
    label: '最大大小',
    help: '下载内容最大大小（MB）。默认 10',
  },
  'media_download.timeout_secs': {
    label: '请求超时',
    help: '下载超时（秒）。默认 30',
  },
  'media_download.use_proxy': {
    label: '使用代理',
    help: '媒体下载是否走代理。默认 false',
  },
  'media_download.proxy_id': {
    label: '代理 UUID',
    help: '指定代理 UUID，留空跟随默认（use_proxy=true 时）。默认空',
  },

  // —— media_poller 媒体轮询 ——
  'media_poller.interval_secs': {
    label: '轮询间隔',
    help: '图片/视频异步任务轮询间隔（秒）。默认 5',
  },
  'media_poller.max_age_hours': {
    label: '最大任务年龄',
    help: '轮询的任务最大年龄（小时），超过视为失败。默认 24',
  },

  // —— proxy 代理（全局）——
  'proxy.default_proxy_id': {
    label: '默认代理 UUID',
    help: '系统默认代理；空=直连；use_proxy=true 且未显式指定 proxy_id 时跟随此值。默认空',
  },
  'proxy.no_proxy': {
    label: '全局直连名单',
    help: '全局直连名单（与所选代理自身 no_proxy 取并集；CIDR/域名，支持 * 后缀；回车添加）。默认 []',
  },
  'proxy.probe_url': {
    label: '代理探测地址',
    help: '代理连通性测试固定探测地址（白名单，防 SSRF）。默认 https://api.github.com',
  },

  // —— update_check 更新检查 ——
  'update_check.enabled': {
    label: '启用更新检查',
    help: '是否启用定时检查更新。默认 false',
  },
  'update_check.repo': {
    label: 'GitHub 仓库',
    help: '如 owner/nextapi；为空时无法检查更新（400）。默认空',
  },
  'update_check.interval_hours': {
    label: '检查间隔',
    help: '定时检查间隔（小时）；0=关闭定时，仅手动检查。默认 24',
  },
  'update_check.use_proxy': {
    label: '使用代理',
    help: 'GitHub 更新检查是否走代理。默认 false',
  },
  'update_check.proxy_id': {
    label: '代理 UUID',
    help: '指定代理 UUID，留空跟随默认（use_proxy=true 时）。默认空',
  },
}

/** 分组（前缀 → 中文组名，渲染顺序） */
export const SETTING_GROUPS: { prefix: string; title: string }[] = [
  { prefix: 'gateway', title: '网关运行参数' },
  { prefix: 'fx_auto_fetch', title: '汇率自动拉取' },
  { prefix: 'price_import', title: '价格表导入' },
  { prefix: 'media_download', title: '媒体下载' },
  { prefix: 'media_poller', title: '媒体轮询' },
  { prefix: 'proxy', title: '代理（全局）' },
  { prefix: 'update_check', title: '更新检查' },
  { prefix: 'server', title: '服务（启动类）' },
  { prefix: 'database', title: '数据库（启动类）' },
]

/** 未收录键的兜底分组标题 */
export const FALLBACK_GROUP_TITLE = '其它（未分组）'

/** 点分键 → 末段短名（如 gateway.display_currency → display_currency） */
export function shortKey(key: string): string {
  const i = key.lastIndexOf('.')
  return i === -1 ? key : key.slice(i + 1)
}
