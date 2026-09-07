// ============================================================================
// 系统设置：键说明字典（P-Settings 自建）
// 依据 config.example.yaml 注释与 M8 契约 §4.9/§5。
// key 为「点分全键」如 gateway.display_currency；未收录键展示兜底为 key 本身。
//
// 元数据结构：
//   label  —— 中文展示名（必填）
//   help   —— 说明性文案（含默认值/单位叙述，保留原文供 tooltip 展示）
//   type   —— 控件类型：'number' | 'boolean' | 'string'
//             （string 含文本/下拉/列表，面板依据运行时值与选项细分）
//   min/max—— 数值上下限；保存前校验（未设置则不校验上限/下限）
//   unit   —— 数值单位（展示用，如 秒/MB/分钟）
//   default—— 默认值（自 help/config.example.yaml 结构化提取），用于「恢复默认」
// ============================================================================

export interface SettingField {
  label: string
  help: string
  /** 控件类型：number | boolean | string（string 覆盖文本/select/tags，按运行时细分） */
  type?: 'number' | 'boolean' | 'string'
  /** 数值下限（type=number 时生效） */
  min?: number
  /** 数值上限（type=number 时生效） */
  max?: number
  /** 数值单位说明（展示用） */
  unit?: string
  /** 默认值；未收录默认值时缺省（跳过「恢复默认」） */
  default?: string | number | boolean | string[]
}

/** 点分全键 → 中文 label/help 及类型/默认值元数据 */
export const SETTING_FIELDS: Record<string, SettingField> = {
  // —— server（启动类）——
  'server.listen': {
    label: '监听地址',
    type: 'string',
    default: '0.0.0.0:8080',
    help: '监听地址（host:port）。环境变量 NEXTAPI_LISTEN 覆盖。启动类，需重启。默认 0.0.0.0:8080',
  },
  'server.admin_jwt_secret': {
    label: 'JWT 签名密钥',
    type: 'string',
    default: '',
    help: '管理端 JWT 签名密钥，生产必填。敏感字段；环境变量 NEXTAPI_ADMIN_JWT_SECRET 覆盖。启动类，需重启。默认空（非 debug 模式拒绝启动）',
  },
  'server.debug': {
    label: '调试模式',
    type: 'boolean',
    default: false,
    help: '放宽启动检查，仅本地开发使用。启动类，需重启。默认 false',
  },

  // —— database（启动类）——
  'database.url': {
    label: '数据库连接串',
    type: 'string',
    default: 'postgres://nextapi:nextapi@localhost:5432/nextapi',
    help: 'Postgres 连接串。启动类，需重启；优先级 env DATABASE_URL > UI > YAML。默认 postgres://nextapi:nextapi@localhost:5432/nextapi',
  },

  // —— gateway 网关运行参数 ——
  'gateway.default_rate_limit_rpm': {
    label: '默认限流 RPM',
    type: 'number',
    min: 0,
    unit: '次/分钟',
    default: 600,
    help: '每个 Key 每分钟请求数上限。默认 600',
  },
  'gateway.default_timeout_secs': {
    label: '默认上游超时',
    type: 'number',
    min: 0,
    unit: '秒',
    default: 300,
    help: '非流式请求上游超时（秒）。默认 300',
  },
  'gateway.stream_timeout_secs': {
    label: '流式上游超时',
    type: 'number',
    min: 0,
    unit: '秒',
    default: 600,
    help: '流式请求上游超时（秒）。默认 600',
  },
  'gateway.billing_timezone': {
    label: '计费时区',
    type: 'string',
    default: 'Asia/Shanghai',
    help: '计费时区（分时段/窗口起点/日志日界）。默认 Asia/Shanghai',
  },
  'gateway.display_currency': {
    label: '展示币种',
    type: 'string',
    default: 'CNY',
    help: '混合统计默认展示币种（CNY|USD）。默认 CNY',
  },
  'gateway.display_precision': {
    label: '展示精度',
    type: 'number',
    min: 0,
    max: 10,
    default: 6,
    help: '金额展示舍入小数位（DB 仍 NUMERIC(20,10)）。默认 6',
  },
  'gateway.fx_stale_max_minutes': {
    label: '汇率过期阈值',
    type: 'number',
    min: 0,
    unit: '分钟',
    default: 1440,
    help: '最近成功汇率超过该分钟数视为过期不可用。默认 1440',
  },
  'gateway.log_async': {
    label: '异步写库',
    type: 'boolean',
    default: true,
    help: '异步写库；false=同步（调试用）。默认 true。仅启动时生效，改动需重启。',
  },
  'gateway.log_queue_capacity': {
    label: '日志队列容量',
    type: 'number',
    min: 0,
    unit: '条',
    default: 10000,
    help: '有界队列容量；溢出先写 WAL 再丢弃内存条目。默认 10000。仅启动时生效，改动需重启。',
  },
  'gateway.log_wal_dir': {
    label: 'WAL 目录',
    type: 'string',
    default: '/data/wal',
    help: 'WAL 落盘目录。默认 /data/wal',
  },
  'gateway.log_wal_file_max_mb': {
    label: 'WAL 单文件上限',
    type: 'number',
    min: 0,
    unit: 'MB',
    default: 128,
    help: 'WAL 单文件大小上限（MB）。默认 128',
  },
  'gateway.log_wal_archive_tail_mb': {
    label: 'WAL 归档尾部',
    type: 'number',
    min: 0,
    unit: 'MB',
    default: 64,
    help: '尾部 N MB 全部写入并重放完成即归档。默认 64',
  },
  'gateway.log_partition_days': {
    label: '日志分区天数',
    type: 'number',
    min: 0,
    unit: '天',
    default: 30,
    help: 'usage_logs 每分区覆盖天数。首次建分区时固化到 app_meta，此后以固化值为准；改此项仅对尚未建过分区的空库生效。默认 30',
  },
  'gateway.log_detail_retention_days': {
    label: '明细保留天数',
    type: 'number',
    min: 0,
    unit: '天',
    default: 0,
    help: '明细不自动清理；仅作手动清理默认 before（0=永久）。默认 0',
  },
  'gateway.log_aggregate_retention_days': {
    label: '汇总保留天数',
    type: 'number',
    min: 0,
    unit: '天',
    default: 365,
    help: '汇总表保留建议值，不自动删除（0=永久）。默认 365',
  },
  'gateway.log_debug_ttl_minutes': {
    label: 'Debug 最长时长',
    type: 'number',
    min: 0,
    unit: '分钟',
    default: 60,
    help: '按 Key debug 模式最长开启时长（分钟）。默认 60',
  },
  'gateway.admin_login_rate_limit_per_min': {
    label: '登录限速',
    type: 'number',
    min: 0,
    unit: '次/分钟',
    default: 10,
    help: '登录接口防爆破限速（次/分钟）。默认 10',
  },
  'gateway.anthropic_default_max_tokens': {
    label: 'Anthropic 默认 max_tokens',
    type: 'number',
    min: 0,
    unit: 'tokens',
    default: 65536,
    help: '转 Anthropic 缺省 max_tokens（64k）。默认 65536',
  },
  'gateway.batch_insert_interval_ms': {
    label: '批量写库间隔',
    type: 'number',
    min: 0,
    unit: '毫秒',
    default: 500,
    help: '批量写库批次间隔（毫秒）。默认 500',
  },
  'gateway.quota_check_cache_secs': {
    label: '用量缓存秒数',
    type: 'number',
    min: 0,
    unit: '秒',
    default: 3,
    help: '用量上限聚合结果缓存秒数。默认 3',
  },
  'gateway.sticky_routing': {
    label: 'Key 粘性路由',
    type: 'boolean',
    default: false,
    help: '同一 Key 同模型固定落到最近成功的上游（便于利用上游缓存）；pinned 上游不可用自动回落加权随机。默认 false',
  },
  'gateway.quota_exceed_action': {
    label: '超配额动作',
    type: 'string',
    default: 'block',
    help: 'block=429 硬阻断 | warn=仅日志告警。默认 block',
  },

  // —— fx_auto_fetch 汇率自动拉取 ——
  'fx_auto_fetch.enabled': {
    label: '启用自动汇率',
    type: 'boolean',
    default: true,
    help: '是否启用外部汇率自动拉取（source=auto）。默认 true',
  },
  'fx_auto_fetch.provider': {
    label: '汇率数据源',
    type: 'string',
    default: 'frankfurter',
    help: 'frankfurter | ecb | custom。默认 frankfurter',
  },
  'fx_auto_fetch.base': {
    label: '基准币种',
    type: 'string',
    default: 'USD',
    help: '拉取汇率的基准货币。默认 USD',
  },
  'fx_auto_fetch.symbols': {
    label: '目标币种',
    type: 'string',
    default: ['CNY'],
    help: '需要兑换的目标币种列表（回车添加）。默认 [CNY]',
  },
  'fx_auto_fetch.refresh_interval_minutes': {
    label: '刷新间隔',
    type: 'number',
    min: 0,
    unit: '分钟',
    default: 60,
    help: '自动拉取间隔（分钟）。默认 60',
  },
  'fx_auto_fetch.cache_ttl_minutes': {
    label: '缓存有效期',
    type: 'number',
    min: 0,
    unit: '分钟',
    default: 60,
    help: '汇率缓存 TTL（分钟）。默认 60',
  },
  'fx_auto_fetch.timeout_secs': {
    label: '请求超时',
    type: 'number',
    min: 0,
    unit: '秒',
    default: 10,
    help: '拉取汇率超时（秒）。默认 10',
  },
  'fx_auto_fetch.use_proxy': {
    label: '使用代理',
    type: 'boolean',
    default: false,
    help: '外联代理矩阵：拉取汇率是否走代理。默认 false',
  },
  'fx_auto_fetch.proxy_id': {
    label: '代理 UUID',
    type: 'string',
    default: '',
    help: '指定代理 UUID，留空跟随默认（use_proxy=true 时）。默认空',
  },

  // —— price_import 价格表导入 ——
  'price_import.allow_url': {
    label: '允许 URL 导入',
    type: 'boolean',
    default: true,
    help: '是否允许从 URL 导入价格表（仅 https，SSRF 防护）。默认 true',
  },
  'price_import.max_size_mb': {
    label: '最大大小',
    type: 'number',
    min: 0,
    unit: 'MB',
    default: 1,
    help: '导入内容最大大小（MB）。默认 1',
  },
  'price_import.timeout_secs': {
    label: '请求超时',
    type: 'number',
    min: 0,
    unit: '秒',
    default: 10,
    help: '导入拉取超时（秒）。默认 10',
  },
  'price_import.use_proxy': {
    label: '使用代理',
    type: 'boolean',
    default: false,
    help: '价格 URL 导入是否走代理。默认 false',
  },
  'price_import.proxy_id': {
    label: '代理 UUID',
    type: 'string',
    default: '',
    help: '指定代理 UUID，留空跟随默认（use_proxy=true 时）。默认空',
  },

  // —— media_download 媒体下载 ——
  'media_download.enabled': {
    label: '启用媒体下载',
    type: 'boolean',
    default: false,
    help: '媒体 URL 下载转内联（默认关闭，SSRF 防护）。默认 false',
  },
  'media_download.max_size_mb': {
    label: '最大大小',
    type: 'number',
    min: 0,
    unit: 'MB',
    default: 10,
    help: '下载内容最大大小（MB）。默认 10',
  },
  'media_download.timeout_secs': {
    label: '请求超时',
    type: 'number',
    min: 0,
    unit: '秒',
    default: 30,
    help: '下载超时（秒）。默认 30',
  },
  'media_download.use_proxy': {
    label: '使用代理',
    type: 'boolean',
    default: false,
    help: '媒体下载是否走代理。默认 false',
  },
  'media_download.proxy_id': {
    label: '代理 UUID',
    type: 'string',
    default: '',
    help: '指定代理 UUID，留空跟随默认（use_proxy=true 时）。默认空',
  },

  // —— media_poller 媒体轮询 ——
  'media_poller.interval_secs': {
    label: '轮询间隔',
    type: 'number',
    min: 0,
    unit: '秒',
    default: 5,
    help: '图片/视频异步任务轮询间隔（秒）。默认 5',
  },
  'media_poller.max_age_hours': {
    label: '最大任务年龄',
    type: 'number',
    min: 0,
    unit: '小时',
    default: 24,
    help: '轮询的任务最大年龄（小时），超过视为失败。默认 24',
  },

  // —— proxy 代理（全局）——
  'proxy.default_proxy_id': {
    label: '默认代理 UUID',
    type: 'string',
    default: '',
    help: '系统默认代理；空=直连；use_proxy=true 且未显式指定 proxy_id 时跟随此值。默认空',
  },
  'proxy.no_proxy': {
    label: '全局直连名单',
    type: 'string',
    default: [],
    help: '全局直连名单（与所选代理自身 no_proxy 取并集；CIDR/域名，支持 * 后缀；回车添加）。默认 []',
  },
  'proxy.probe_url': {
    label: '代理探测地址',
    type: 'string',
    default: 'https://api.github.com',
    help: '代理连通性测试固定探测地址（白名单，防 SSRF）。默认 https://api.github.com',
  },

  // —— update_check 更新检查 ——
  'update_check.enabled': {
    label: '启用更新检查',
    type: 'boolean',
    default: false,
    help: '是否启用定时检查更新。默认 false',
  },
  'update_check.repo': {
    label: 'GitHub 仓库',
    type: 'string',
    default: '',
    help: '如 owner/nextapi；为空时无法检查更新（400）。默认空',
  },
  'update_check.interval_hours': {
    label: '检查间隔',
    type: 'number',
    min: 0,
    unit: '小时',
    default: 24,
    help: '定时检查间隔（小时）；0=关闭定时，仅手动检查。默认 24',
  },
  'update_check.use_proxy': {
    label: '使用代理',
    type: 'boolean',
    default: false,
    help: 'GitHub 更新检查是否走代理。默认 false',
  },
  'update_check.proxy_id': {
    label: '代理 UUID',
    type: 'string',
    default: '',
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
