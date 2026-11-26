<template>
  <div class="section-body">
    <div class="section-head">
      <div class="section-head-main">
        <h2 class="section-title">请求日志</h2>
        <span class="hint">默认今天；筛选条件会写入网址，可直接分享。</span>
      </div>
      <div class="section-head-actions">
        <el-tooltip
          content="按当前筛选导出，最多 10 万行"
          placement="top"
        >
          <el-button
            type="primary"
            plain
            :icon="Download"
            :loading="exporting"
            title="CSV 导出"
            aria-label="CSV 导出"
            @click="exportCsv"
          >
            <span v-if="!isMobile">CSV 导出</span>
          </el-button>
        </el-tooltip>
        <el-dropdown trigger="click" @command="onLogMoreCommand">
          <el-button>
            更多<el-icon class="el-icon--right"><ArrowDown /></el-icon>
          </el-button>
          <template #dropdown>
            <el-dropdown-menu>
              <el-dropdown-item command="cursor" :icon="Sort">
                {{ cursorMode ? '退出连续浏览' : '连续浏览' }}
              </el-dropdown-item>
              <el-dropdown-item command="cleanup" :icon="Delete" divided>
                清理旧日志
              </el-dropdown-item>
            </el-dropdown-menu>
          </template>
        </el-dropdown>
      </div>
    </div>

    <!-- 筛选工具栏：默认保留 时间/模型/request_id，Key/上游/状态码/流式/降级 归入「高级筛选」（默认折叠） -->
    <el-card shadow="never" class="section-card filter-card">
      <!-- 移动端：默认收起，只留「预设 · 结果数」与筛选/查询 -->
      <div v-if="isMobile" class="filter-mobile-bar">
        <span class="filter-meta">{{ presetLabel }} · {{ resultCountLabel }}</span>
        <span class="spacer" />
        <el-button class="filter-toggle" @click="mobileFilterOpen = !mobileFilterOpen">
          筛选
          <span v-if="activeFilterCount" class="filter-count">{{ activeFilterCount }}</span>
          <el-icon class="filter-caret" :class="{ 'is-open': mobileFilterOpen }"><ArrowDown /></el-icon>
        </el-button>
        <el-button v-if="!mobileFilterOpen" type="primary" :loading="loading" @click="onQuery">查询</el-button>
      </div>

      <div v-show="!isMobile || mobileFilterOpen">
      <!-- 时间：预设 + 实际区间 -->
      <div class="filter-row">
        <el-select v-model="rangePreset" class="filter-preset" aria-label="时间范围" @change="applyPreset">
          <el-option label="今日" value="today" />
          <el-option label="近 7 天" value="7d" />
          <el-option label="近 30 天" value="30d" />
          <el-option label="近 90 天" value="90d" />
          <el-option label="全部时间" value="all" />
          <el-option label="自定义" value="custom" />
        </el-select>
        <el-date-picker
          v-model="range"
          class="filter-range"
          type="datetimerange"
          :shortcuts="timeShortcuts"
          :format="isMobile ? 'MM-DD HH:mm' : 'MM-DD HH:mm'"
          range-separator="~"
          start-placeholder="开始时间"
          end-placeholder="结束时间"
          clearable
        />
      </div>

      <!-- 结果 / 模型 + 操作：移动端两列，底部只留重置 -->
      <div class="filter-row is-main is-main-2col">
        <span v-if="!isMobile" class="filter-meta">{{ resultCountLabel }}</span>
        <el-select v-model="statusGroup" class="filter-select" clearable placeholder="全部结果" aria-label="结果" @change="onQuery">
          <el-option label="成功" value="success" />
          <el-option label="客户端错误" value="4xx" />
          <el-option label="服务端错误" value="5xx" />
          <el-option label="限流 429" value="429" />
          <el-option label="已降级" value="degraded" />
        </el-select>
        <el-input v-model="model" class="filter-input" placeholder="模型" clearable @keyup.enter="onQuery" />
        <span class="spacer" />
        <template v-if="!isMobile">
          <el-button type="primary" :loading="loading" @click="onQuery">查询</el-button>
          <el-button :disabled="!hasAnyFilter" @click="onReset">重置</el-button>
        </template>
        <el-tooltip v-if="!isMobile" content="请求 ID、密钥、上游、状态码、流式、降级" placement="top">
          <el-button
            link
            class="adv-toggle"
            :type="hasAdvancedFilter() ? 'primary' : 'info'"
            @click="advancedOpen = !advancedOpen"
          >
            高级筛选
            <span v-if="advancedFilterCount" class="adv-count">{{ advancedFilterCount }}</span>
            <el-icon :class="{ 'is-open': advancedOpen }"><ArrowDown /></el-icon>
          </el-button>
        </el-tooltip>
      </div>

      <el-collapse-transition>
        <div v-show="advancedOpen" class="filter-bar advanced-bar">
          <el-input v-model="requestId" class="filter-input" placeholder="request_id" clearable @keyup.enter="onQuery" />
          <el-select v-model="keyId" class="filter-select" filterable clearable placeholder="全部密钥" aria-label="密钥">
            <el-option v-for="k in keys" :key="k.id" :label="keyLabel(k)" :value="k.id" />
          </el-select>
          <el-select v-model="upstreamId" class="filter-select" filterable clearable placeholder="全部上游" aria-label="上游">
            <el-option v-for="u in upstreams" :key="u.id" :label="u.name" :value="u.id" />
          </el-select>
          <el-input v-model="statusStr" class="filter-input" type="number" placeholder="精确状态码" clearable @keyup.enter="onQuery" />
          <el-select v-model="streamOpt" class="filter-select" aria-label="流式">
            <el-option label="流式：全部" value="" />
            <el-option label="流式：是" value="true" />
            <el-option label="流式：否" value="false" />
          </el-select>
          <el-select v-model="degradedOpt" class="filter-select" aria-label="降级">
            <el-option label="降级：全部" value="" />
            <el-option label="降级：是" value="true" />
            <el-option label="降级：否" value="false" />
          </el-select>
        </div>
      </el-collapse-transition>

      <!-- 移动端：高级筛选入口 + 操作行，都在展开区底部 -->
      <div v-if="isMobile" class="filter-row filter-adv-line">
        <el-button link class="adv-toggle" :type="hasAdvancedFilter() ? 'primary' : 'info'" @click="advancedOpen = !advancedOpen">
          高级筛选
          <span v-if="advancedFilterCount" class="filter-count">{{ advancedFilterCount }}</span>
          <el-icon :class="{ 'is-open': advancedOpen }"><ArrowDown /></el-icon>
        </el-button>
      </div>
      <div v-if="isMobile" class="filter-actions-mobile">
        <el-button :disabled="!hasAnyFilter" @click="onReset">重置</el-button>
        <el-button type="primary" :loading="loading" @click="onQuery">查询</el-button>
      </div>
      </div>
    </el-card>

    <!-- 日志表格 -->
    <el-card shadow="never" class="section-card">
      <div class="table-head">
        <span>请求日志</span>
        <div class="spacer" />
        <span v-if="lastLoadedAt" class="hint">更新于 {{ fmtTime(lastLoadedAt) }}</span>
        <el-tooltip
          content="日志异步写入，最近几秒可能尚未入库。Token 列：P 输入、C 输出、Cache 缓存读"
          placement="top"
        >
          <el-icon class="help-icon"><QuestionFilled /></el-icon>
        </el-tooltip>
      </div>
      <template v-if="listError">
        <div v-loading="loading">
          <ErrorState title="请求日志加载失败" :message="listError" @retry="loadLogs" />
        </div>
      </template>
      <template v-else>
      <el-table
        v-if="!isMobile && !isCompact"
        :data="cursorMode ? cursorRows : rows"
        v-loading="loading"
        class="log-table"
        size="small"
        @row-click="openDetail"
      >
        <template #empty>
          <div v-if="loading" class="log-skeleton" aria-busy="true" aria-label="日志加载中">
            <div v-for="i in 6" :key="i" class="log-skeleton-row">
              <SkeletonBlock width="88px" height="13" />
              <SkeletonBlock width="34px" height="13" />
              <SkeletonBlock width="96px" height="13" />
              <SkeletonBlock :width="i % 2 ? '44%' : '62%'" height="13" />
              <SkeletonBlock width="68px" height="13" />
            </div>
          </div>
          <EmptyState v-else title="没有匹配的日志" description="换个时间范围或筛选条件再试" />
        </template>
        <el-table-column label="时间" width="125">
          <template #default="{ row }">
            <div class="time-cell">
              <span class="time-main" :title="fmtTime(row.ts)">{{ fmtLogTime(row.ts) }}</span>
              <span v-if="row.request_id" class="time-rid mono" :title="row.request_id">
                #{{ row.request_id.slice(-8) }}
              </span>
            </div>
          </template>
        </el-table-column>
        <el-table-column label="模型" min-width="160">
          <template #default="{ row }">
            <div class="model-cell">
              <span class="model-main">{{ row.model || '-' }}</span>
              <span v-if="row.upstream_model && row.upstream_model !== row.model" class="hint">
                上游 {{ row.upstream_model }}
              </span>
            </div>
          </template>
        </el-table-column>
        <el-table-column v-if="!isTablet" label="Key" width="120" show-overflow-tooltip>
          <template #default="{ row }">{{ keyText(row) }}</template>
        </el-table-column>
        <el-table-column label="上游 / 协议" min-width="110">
          <template #default="{ row }">
            <div class="model-cell">
              <span class="model-main">{{ row.upstream_name || '-' }}</span>
              <span class="hint proto-line">
                {{ protoLabel(row.protocol_in) }} → {{ protoLabel(row.protocol_out) }}
                <el-tag
                  v-if="row.convert_mode !== 'passthrough'"
                  size="small"
                  type="warning"
                  effect="plain"
                  class="convert-tag"
                >
                  转换
                </el-tag>
              </span>
              <span v-if="row.error" class="row-error" :title="row.error">{{ row.error }}</span>
            </div>
          </template>
        </el-table-column>
        <el-table-column label="状态" width="118" align="center">
          <template #default="{ row }">
            <div class="status-cell">
              <el-tooltip :disabled="!row.error" :content="row.error || ''" placement="top">
                <el-tag effect="plain" :type="statusType(row.status)" size="small" :class="{ 'has-error': !!row.error }">
                  {{ row.status }}
                </el-tag>
              </el-tooltip>
              <el-tooltip
                v-if="row.degraded"
                content="该请求被降级，详情可看具体环节"
                placement="top"
              >
                <el-tag size="small" type="warning" effect="plain">降级</el-tag>
              </el-tooltip>
            </div>
            <div class="status-sub hint">
              {{ row.stream ? '流式' : '非流式' }}
              <span v-if="row.retry_count"> · 重试 {{ row.retry_count }}</span>
              <span v-if="isTablet"> · {{ fmtDur(row.latency_ms) }}</span>
            </div>
          </template>
        </el-table-column>
        <el-table-column v-if="!isTablet" label="延迟" width="96" align="right">
          <template #default="{ row }">
            <div>{{ fmtDur(row.latency_ms) }}</div>
            <div v-if="row.ttfb_ms != null" class="hint">TTFB {{ fmtDur(row.ttfb_ms) }}</div>
          </template>
        </el-table-column>
        <el-table-column v-if="!isTablet" label="Token" min-width="135" align="right">
          <template #default="{ row }">
            <div class="token-cell">
              <span>P {{ fmtTokens(row.prompt_tokens) }}</span>
              <span>C {{ fmtTokens(row.completion_tokens) }}</span>
              <span>Cache {{ fmtTokens(row.cache_read_tokens) }}</span>
            </div>
          </template>
        </el-table-column>
        <el-table-column label="成本" width="110" align="right">
          <template #default="{ row }">
            <div v-if="row.cost_cny != null || row.cost_usd != null" class="cost-cell">
              <div>¥ {{ fmtMoney(row.cost_cny) }}</div>
              <div>$ {{ fmtMoney(row.cost_usd) }}</div>
            </div>
            <span v-else class="hint">未计价</span>
          </template>
        </el-table-column>
        <el-table-column label="操作" width="60" fixed="right">
          <template #default="{ row }">
            <el-button size="small" link type="primary" @click.stop="openDetail(row)">详情</el-button>
          </template>
        </el-table-column>
      </el-table>

      <!-- 窄屏/移动端：日志卡片列表，避免多列表格横向滚动 -->
      <div v-else v-loading="loading && displayRows.length > 0" class="log-cards">
        <div v-if="firstLoad" class="log-skeleton" aria-busy="true" aria-label="日志加载中">
          <div v-for="i in 4" :key="i" class="log-skeleton-card">
            <SkeletonBlock width="56%" height="13" />
            <SkeletonBlock width="78%" height="13" />
            <SkeletonBlock width="40%" height="13" />
          </div>
        </div>
        <EmptyState v-else-if="!displayRows.length" title="没有匹配的日志" description="换个时间范围或筛选条件再试" />
        <article
          v-for="row in displayRows"
          :key="row.request_id"
          class="log-card"
          tabindex="0"
          @click="openDetail(row)"
          @keydown.enter="openDetail(row)"
        >
          <div class="log-card-head">
            <el-tag effect="plain" :type="statusType(row.status)" size="small">{{ row.status }}</el-tag>
            <span class="log-card-model">{{ row.model || '-' }}</span>
            <div class="spacer" />
            <span class="log-card-cost">
              <template v-if="row.cost_cny != null || row.cost_usd != null">
                ¥ {{ fmtMoney(row.cost_cny) }}
              </template>
              <template v-else>未计价</template>
            </span>
          </div>
          <div class="log-card-meta hint">
            <div class="log-card-meta-row">
              <span>{{ fmtLogTime(row.ts) }}</span>
              <span v-if="row.request_id" class="mono" :title="row.request_id">#{{ row.request_id.slice(-8) }}</span>
            </div>
            <div class="log-card-meta-row">
              <span>{{ row.upstream_name || '未知上游' }}</span>
              <span>{{ fmtDur(row.latency_ms) }}</span>
              <span v-if="row.stream">流式</span>
              <el-tag v-if="row.degraded" size="small" type="warning" effect="plain">降级</el-tag>
            </div>
          </div>
          <div v-if="row.error" class="log-card-error" :title="row.error">{{ row.error }}</div>
          <div class="log-card-foot">
            <span class="hint">
              P {{ fmtTokens(row.prompt_tokens) }} · C {{ fmtTokens(row.completion_tokens) }} · Cache
              {{ fmtTokens(row.cache_read_tokens) }}
            </span>
            <div class="spacer" />
            <el-button size="small" link type="primary" @click.stop="openDetail(row)">详情</el-button>
          </div>
        </article>
      </div>
        <div v-if="cursorMode" class="cursor-foot">
          <span class="hint">已加载 {{ cursorRows.length }} 条（不统计总数）</span>
          <el-button v-if="cursorNext" type="primary" plain :loading="cursorLoading" @click="loadMoreCursor">
            加载更多
          </el-button>
          <span v-else class="hint">没有更多了</span>
        </div>
        <TablePager
          v-else
          v-model:page="currentPage"
          v-model:size="pageSize"
          :total="total"
          :page-sizes="[20, 50, 100]"
          class="pagination"
          @update:page="onPageChange"
          @update:size="onSizeChange"
        />
      </template>
    </el-card>

    <!-- 详情抽屉 -->
    <el-drawer v-model="detailVisible" title="请求详情" :size="drawerSize">
      <div v-loading="detailLoading">
        <template v-if="detail">
          <!-- 1. 请求结果 -->
          <div class="detail-head">
            <el-tag effect="plain" :type="statusType(detail.status)" size="small">{{ detail.status }}</el-tag>
            <span class="mono rid">{{ detail.request_id }}</span>
            <el-button size="small" link type="primary" @click="copyRequestId(detail.request_id)">
              复制
            </el-button>
          </div>

          <div class="group-title">请求结果</div>
          <el-descriptions :column="descColumn" border>
            <el-descriptions-item label="时间">{{ fmtTime(detail.ts) }}</el-descriptions-item>
            <el-descriptions-item label="状态">
              <el-tag :type="statusType(detail.status)" size="small" effect="plain">{{ detail.status }}</el-tag>
              <el-tag v-if="detail.degraded" size="small" type="warning" effect="plain" class="tag-gap">
                降级
              </el-tag>
            </el-descriptions-item>
            <el-descriptions-item label="延迟">{{ fmtDur(detail.latency_ms) }}</el-descriptions-item>
            <el-descriptions-item label="TTFB">{{ fmtDur(detail.ttfb_ms) }}</el-descriptions-item>
            <el-descriptions-item label="重试次数">{{ detail.retry_count }}</el-descriptions-item>
            <el-descriptions-item label="流式">{{ detail.stream ? '是' : '否' }}</el-descriptions-item>
          </el-descriptions>

          <!-- 2. 路由信息 -->
          <div class="group-title">路由信息</div>
          <el-descriptions :column="descColumn" border>
            <el-descriptions-item label="入口模型">
              {{ detail.requested_model || detail.model }}
              <span v-if="detail.requested_model && detail.requested_model !== detail.model" class="hint">
                （别名）
              </span>
            </el-descriptions-item>
            <el-descriptions-item label="实际模型">
              {{ detail.model }}
              <span v-if="detail.upstream_model" class="hint">（上游实际名 {{ detail.upstream_model }}）</span>
            </el-descriptions-item>
            <el-descriptions-item label="上游">{{ detail.upstream_name || '-' }}</el-descriptions-item>
            <el-descriptions-item label="Key">{{ keyText(detail) }}</el-descriptions-item>
            <el-descriptions-item label="协议链">
              {{ protoLabel(detail.protocol_in) }} → {{ protoLabel(detail.protocol_out) }}
            </el-descriptions-item>
            <el-descriptions-item label="转换方式">{{ detail.convert_mode || '-' }}</el-descriptions-item>
          </el-descriptions>

          <!-- 3. 用量与成本 -->
          <div class="group-title">用量与成本</div>
          <el-descriptions :column="descColumn" border>
            <el-descriptions-item label="输入 Token">
              {{ fmtTokens(detail.prompt_tokens) }}
              <el-tooltip content="未缓存的输入；缓存命中量见「缓存读取」" placement="top">
                <el-icon class="help-icon"><QuestionFilled /></el-icon>
              </el-tooltip>
            </el-descriptions-item>
            <el-descriptions-item label="输出 Token">{{ fmtTokens(detail.completion_tokens) }}</el-descriptions-item>
            <el-descriptions-item label="缓存写入">{{ fmtTokens(detail.cache_write_tokens) }}</el-descriptions-item>
            <el-descriptions-item label="缓存读取">{{ fmtTokens(detail.cache_read_tokens) }}</el-descriptions-item>
            <el-descriptions-item v-if="detail.images" label="图片数">{{ fmtInt(detail.images) }}</el-descriptions-item>
            <el-descriptions-item v-if="detail.image_size" label="图片尺寸">{{ detail.image_size }}</el-descriptions-item>
            <el-descriptions-item v-if="detail.video_seconds" label="视频时长">
              {{ detail.video_seconds }}s
            </el-descriptions-item>
            <el-descriptions-item v-if="detail.video_resolution" label="视频分辨率">
              {{ detail.video_resolution }}
            </el-descriptions-item>
            <el-descriptions-item v-if="detail.video_task_type" label="视频任务">
              {{ detail.video_task_type }}
            </el-descriptions-item>
            <el-descriptions-item label="计价来源">
              {{ detail.pricing_source || '未计价' }}
            </el-descriptions-item>
            <el-descriptions-item label="成本">
              <span v-if="detail.cost_cny != null || detail.cost_usd != null">
                ¥ {{ fmtMoney(detail.cost_cny) }} / $ {{ fmtMoney(detail.cost_usd) }}
              </span>
              <span v-else class="hint">未配置价格</span>
            </el-descriptions-item>
          </el-descriptions>

          <!-- 4. 原始记录 -->
          <div class="group-title">原始记录</div>

          <div v-if="detail?.error" class="error-block">Error: {{ detail.error }}</div>

          <el-collapse v-model="detailOpen" class="json-cards">
            <el-collapse-item title="usage_raw" name="usage_raw">
              <pre class="mono">{{ jsonText(detail?.usage_raw) || '-' }}</pre>
            </el-collapse-item>
            <el-collapse-item title="price_used" name="price_used">
              <pre class="mono">{{ jsonText(detail?.price_used) || '-' }}</pre>
            </el-collapse-item>
            <el-collapse-item title="fx_snapshot" name="fx_snapshot">
              <pre class="mono">{{ jsonText(detail?.fx_snapshot) || '-' }}</pre>
            </el-collapse-item>
            <el-collapse-item title="request_headers（白名单）" name="request_headers">
              <pre v-if="!isEmptyJson(detail?.request_headers)" class="mono">{{ jsonText(detail?.request_headers) }}</pre>
              <span v-else class="hint">无记录（只采集 UA 等白名单头）</span>
            </el-collapse-item>
            <el-collapse-item title="debug_payload" name="debug_payload">
              <pre v-if="!isEmptyJson(detail?.debug_payload)" class="mono">{{ jsonText(detail?.debug_payload) }}</pre>
              <span v-else class="hint">该密钥未开调试，或记录已过期</span>
            </el-collapse-item>
          </el-collapse>
        </template>
        <ErrorState v-else-if="detailError" title="详情加载失败" :message="detailError" @retry="retryDetail" />
        <el-empty v-else-if="!detailLoading" description="无详情" :image-size="80" />
      </div>
    </el-drawer>

    <!-- 清理旧日志弹窗 -->
    <el-dialog v-model="cleanupVisible" title="清理旧日志" class="dlg" :close-on-click-modal="false">
      <div class="cleanup-desc hint">
        日志按 30 天一个分区存储。清理只能整分区删除，不会动未到期的分区，系统也不会自动清理。
        建议先预览要删的分区，确认后再删除。
      </div>
      <div class="f-item" style="margin-bottom: 6px">
        <span class="f-label">截止时间</span>
        <el-date-picker
          class="w-260"
          v-model="cleanupBefore"
          type="datetime"
          placeholder="选择分区截止时间"
          :clearable="false"
          />
      </div>
      <div class="hint" style="margin-bottom: 10px">
        实际截止时间会按分区边界向前对齐；配额计数不受影响。
      </div>

      <div v-if="cleanupPreviewDone">
        <el-descriptions v-if="cleanupEffectiveBefore" :column="2" border size="small" class="cleanup-summary">
          <el-descriptions-item label="实际生效截止">
            {{ fmtTime(cleanupEffectiveBefore) }}
          </el-descriptions-item>
          <el-descriptions-item label="待删分区">{{ cleanupPreview.length }} 个</el-descriptions-item>
          <el-descriptions-item label="待删明细行数">
            {{ fmtInt(cleanupSummary?.log_rows ?? 0) }}
          </el-descriptions-item>
          <el-descriptions-item label="待删聚合行数">{{ fmtInt(cleanupHourlyRows) }}</el-descriptions-item>
          <el-descriptions-item label="Token 合计">
            输入 {{ fmtTokens(cleanupSummary?.prompt_tokens ?? 0) }} · 输出
            {{ fmtTokens(cleanupSummary?.completion_tokens ?? 0) }}
          </el-descriptions-item>
          <el-descriptions-item label="成本合计">
            ¥ {{ fmtMoney(cleanupSummary?.cost_cny ?? '0') }} / $ {{ fmtMoney(cleanupSummary?.cost_usd ?? '0') }}
          </el-descriptions-item>
        </el-descriptions>
        <el-table v-if="cleanupPreview.length" :data="cleanupPreview" size="small" max-height="260">
          <el-table-column prop="name" label="分区" min-width="180" show-overflow-tooltip />
          <el-table-column label="时间范围" min-width="230">
            <template #default="{ row }">{{ fmtTime(row.from_ts) }} ~ {{ fmtTime(row.to_ts) }}</template>
          </el-table-column>
          <el-table-column label="大小" width="100" align="right">
            <template #default="{ row }">{{ fmtBytes(row.size_bytes) }}</template>
          </el-table-column>
          <el-table-column label="行数" width="110" align="right">
            <template #default="{ row }">{{ fmtInt(row.row_estimate) }}</template>
          </el-table-column>
        </el-table>
        <el-empty v-else description="没有可清理的分区" :image-size="80" />
      </div>

      <template #footer>
        <el-button @click="cleanupVisible = false">取消</el-button>
        <el-button type="primary" :loading="cleanupLoading" @click="runCleanupPreview">
          预览（dry-run）
        </el-button>
        <el-button
          v-if="cleanupPreview.length"
          type="danger"
          :loading="cleanupExecuting"
          @click="confirmCleanup"
        >
          删除 {{ cleanupPreview.length }} 个分区
        </el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, nextTick, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { ArrowDown, Delete, Download, QuestionFilled, Sort } from '@element-plus/icons-vue'
import dayjs from 'dayjs'
import { useRoute, useRouter } from 'vue-router'
import { keyApi, logApi, upstreamApi, type LogListQuery } from '@/api'
import { errMsg } from '@/api/http'
import type { ApiKeyRow, CleanupSummary, LogItem, PartitionInfo, UpstreamOut } from '@/api/types'
import { PROTOCOL_IN_LABELS, statusType } from '@/utils/consts'
import { downloadBlob } from '@/utils/download'
import { fmtBytes, fmtDur, fmtInt, fmtMoney, fmtTime, fmtTokens, todayRange } from '@/utils/format'
import { RANGE_PRESET_LABELS } from '@/utils/consts'
import { confirmDanger } from '@/utils/confirm'
import { copyText } from '@/utils/clipboard'
import ErrorState from '@/components/common/ErrorState.vue'
import EmptyState from '@/components/common/EmptyState.vue'
import TablePager from '@/components/common/TablePager.vue'
import SkeletonBlock from '@/components/common/SkeletonBlock.vue'
import { useBreakpoint } from '@/composables/useBreakpoint'

type TriState = '' | 'true' | 'false'

const timeShortcuts = [
  {
    text: '今天',
    value: () => [dayjs().startOf('day').toDate(), dayjs().toDate()],
  },
  {
    text: '昨天',
    value: () => [
      dayjs().startOf('day').subtract(1, 'day').toDate(),
      dayjs().endOf('day').subtract(1, 'day').toDate(),
    ],
  },
  {
    text: '近 7 天',
    value: () => [dayjs().subtract(7, 'day').toDate(), dayjs().toDate()],
  },
  {
    text: '近 30 天',
    value: () => [dayjs().subtract(30, 'day').toDate(), dayjs().toDate()],
  },
]

// —— 路由（筛选同步到 URL query）——
const route = useRoute()
const router = useRouter()

// —— 筛选状态 ——
/** 默认时间范围：今天 00:00 ~ 现在（用户清空后仍可恢复服务端默认语义） */
function defaultRange(): [Date, Date] {
  const [s, e] = todayRange()
  return [new Date(s), new Date(e)]
}

const range = ref<[Date, Date] | null>(defaultRange())

/** 时间范围预设：筛选条保持一横条，自定义区间才展开宽日期控件 */
type RangePreset = 'today' | '7d' | '30d' | '90d' | 'all' | 'custom'
const rangePreset = ref<RangePreset>('today')
let programmaticRange = false

function applyPreset(v: string | number | boolean | undefined) {
  const preset = typeof v === 'string' ? (v as RangePreset) : 'today'
  rangePreset.value = preset
  if (preset === 'custom') return
  programmaticRange = true
  if (preset === 'all') range.value = null
  else if (preset === 'today') range.value = [dayjs().startOf('day').toDate(), dayjs().toDate()]
  else {
    const days = preset === '7d' ? 7 : preset === '30d' ? 30 : 90
    range.value = [dayjs().subtract(days, 'day').toDate(), dayjs().toDate()]
  }
  void nextTick(() => {
    programmaticRange = false
  })
}

watch(
  range,
  () => {
    if (!programmaticRange) rangePreset.value = 'custom'
  },
  { deep: true },
)
const keyId = ref('')
const upstreamId = ref('')
const model = ref('')
const statusGroup = ref('')
const requestId = ref('')
const statusStr = ref('')
const streamOpt = ref<TriState>('')
const degradedOpt = ref<TriState>('')
/** 高级筛选（Key/上游/状态码/流式/降级）是否展开；默认收起，URL 带高级条件时自动展开 */
const advancedOpen = ref(false)

/** 是否存在任意高级筛选条件（用于 URL 恢复自动展开与按钮态提示） */
function hasAdvancedFilter(): boolean {
  return (
    keyId.value !== '' ||
    upstreamId.value !== '' ||
    statusStr.value !== '' ||
    streamOpt.value !== '' ||
    degradedOpt.value !== ''
  )
}

/** 主/高级筛选任一非默认 → 才允许重置 */
/** 移动端筛选区展开状态（默认收起） */
const mobileFilterOpen = ref(false)
const presetLabel = computed(() => RANGE_PRESET_LABELS[rangePreset.value] ?? '自定义区间')
/** 生效条件总数：主筛选 + 高级筛选，收起时也能看出有几条 */
const activeFilterCount = computed(
  () => advancedFilterCount.value + (model.value !== '' ? 1 : 0) + (statusGroup.value !== '' ? 1 : 0) + (rangePreset.value !== 'today' ? 1 : 0),
)

const hasAnyFilter = computed(
  () => hasAdvancedFilter() || model.value !== '' || statusGroup.value !== '' || rangePreset.value !== 'today',
)

/** 结果统计：分页模式看总数，游标模式只能报已加载条数 */
const resultCountLabel = computed(() =>
  cursorMode.value ? `已加载 ${fmtInt(cursorRows.value.length)} 条` : `共 ${fmtInt(total.value)} 条`,
)

/** 已启用的高级筛选条件数（按钮上显示，避免收起的条件被遗忘） */
const advancedFilterCount = computed(
  () => [keyId.value, upstreamId.value, statusStr.value, streamOpt.value, degradedOpt.value].filter((v) => v !== '').length,
)

// —— 列表状态 ——
const rows = ref<LogItem[]>([])
/** 表格/卡片当前数据：页码模式 rows，游标模式 cursorRows */
const displayRows = computed(() => (cursorMode.value ? cursorRows.value : rows.value))
const total = ref(0)
const currentPage = ref(1)
const pageSize = ref(20)
const loading = ref(false)
/** 首屏（尚无任何行）加载：用骨架行代替整表遮罩 */
const firstLoad = computed(() => loading.value && displayRows.value.length === 0)
const listError = ref('')
/** 响应式：手机端用卡片列表，平板（≤1280）隐藏低优先列 */
const { isMobile, isCompact, isTablet } = useBreakpoint()

const keys = ref<ApiKeyRow[]>([])
const upstreams = ref<UpstreamOut[]>([])

function tsToIso(d: Date): string {
  return dayjs(d).toISOString()
}

function triToBool(v: TriState): boolean | undefined {
  if (v === 'true') return true
  if (v === 'false') return false
  return undefined
}

function buildFilterQuery(): LogListQuery {
  const q: LogListQuery = {}
  if (range.value) {
    q.from_ts = tsToIso(range.value[0])
    q.to_ts = tsToIso(range.value[1])
  }
  if (keyId.value) q.key_id = keyId.value
  if (upstreamId.value) q.upstream_id = upstreamId.value
  const m = model.value.trim()
  if (m) q.model = m
  if (statusGroup.value) q.status_group = statusGroup.value as NonNullable<LogListQuery['status_group']>
  const rid = requestId.value.trim()
  if (rid) q.request_id = rid
  if (statusStr.value !== '') {
    const n = Number(statusStr.value)
    if (Number.isFinite(n)) q.status = n
  }
  const stream = triToBool(streamOpt.value)
  if (stream !== undefined) q.stream = stream
  const degraded = triToBool(degradedOpt.value)
  if (degraded !== undefined) q.degraded = degraded
  return q
}

function buildQuery(): LogListQuery {
  return { ...buildFilterQuery(), page: currentPage.value, page_size: pageSize.value }
}

let suppressPageEvent = false

// 请求序号：旧响应到达时丢弃，防乱序覆盖新条件的结果（发布审阅前端 M2）
let loadSeq = 0

async function loadLogs() {
  const seq = ++loadSeq
  loading.value = true
  try {
    const resp = await logApi.list(buildQuery())
    if (seq !== loadSeq) return
    rows.value = resp.items
    total.value = resp.total ?? 0
    listError.value = ''
    lastLoadedAt.value = new Date().toISOString()
  } catch (e) {
    if (seq !== loadSeq) return
    rows.value = []
    total.value = 0
    listError.value = errMsg(e)
  } finally {
    if (seq === loadSeq) loading.value = false
  }
}

/** 游标模式（M14 §5.2）：加载首页（不带 cursor），重置已加载列表 */
async function loadCursorFirstPage() {
  const seq = ++loadSeq
  cursorLoading.value = true
  try {
    const resp = await logApi.list({ ...buildFilterQuery(), page_size: pageSize.value })
    if (seq !== loadSeq) return
    cursorRows.value = resp.items
    cursorNext.value = resp.next_cursor ?? null
    listError.value = ''
    lastLoadedAt.value = new Date().toISOString()
  } catch (e) {
    if (seq !== loadSeq) return
    cursorRows.value = []
    cursorNext.value = null
    listError.value = errMsg(e)
  } finally {
    if (seq === loadSeq) cursorLoading.value = false
  }
}

/** 游标模式：追加下一页（keyset 续页，深页不退化） */
async function loadMoreCursor() {
  const cur = cursorNext.value
  if (!cur || cursorLoading.value) return
  const seq = loadSeq
  cursorLoading.value = true
  try {
    const resp = await logApi.list({
      ...buildFilterQuery(),
      page_size: pageSize.value,
      cursor: cur,
    })
    if (seq !== loadSeq) return
    cursorRows.value = [...cursorRows.value, ...resp.items]
    cursorNext.value = resp.next_cursor ?? null
  } catch (e) {
    if (seq !== loadSeq) return
    ElMessage.error(errMsg(e))
  } finally {
    cursorLoading.value = false
  }
}

/** 切换游标浏览 / 页码模式 */
/** 低频操作菜单：游标浏览 / 清理旧日志，避免页头按钮过多 */
function onLogMoreCommand(command: string) {
  if (command === 'cursor') toggleCursorMode()
  else if (command === 'cleanup') openCleanup()
}

function toggleCursorMode() {
  cursorMode.value = !cursorMode.value
  if (cursorMode.value) {
    syncQueryToUrl()
    loadCursorFirstPage()
  } else {
    loadLogs()
  }
}

/** 程序化重置页码（避免触发 pagination 的 current-change 二次加载） */
function resetPage() {
  suppressPageEvent = true
  currentPage.value = 1
  suppressPageEvent = false
}

/** 仅「查询」动作与分页变化时把当前筛选写入 URL query */
function syncQueryToUrl() {
  const f = buildFilterQuery()
  const q: Record<string, string | number> = {}
  if (f.from_ts) q.from_ts = f.from_ts
  if (f.to_ts) q.to_ts = f.to_ts
  if (f.key_id) q.key_id = f.key_id
  if (f.upstream_id) q.upstream_id = f.upstream_id
  if (f.model) q.model = f.model
  if (f.request_id) q.request_id = f.request_id
  if (f.status_group) q.status_group = f.status_group
  if (f.status !== undefined) q.status = f.status
  if (f.stream !== undefined) q.stream = f.stream ? 'true' : 'false'
  if (f.degraded !== undefined) q.degraded = f.degraded ? 'true' : 'false'
  q.page = currentPage.value
  q.page_size = pageSize.value
  // 内容相同则跳过 replace，避免冗余导航
  if (canonicalQuery(q) === canonicalQuery(route.query)) return
  router.replace({ path: route.path, query: q })
}

function onQuery() {
  resetPage()
  syncQueryToUrl()
  if (cursorMode.value) loadCursorFirstPage()
  else loadLogs()
}

function onReset() {
  range.value = defaultRange()
  keyId.value = ''
  upstreamId.value = ''
  model.value = ''
  requestId.value = ''
  statusGroup.value = ''
  statusStr.value = ''
  streamOpt.value = ''
  degradedOpt.value = ''
  advancedOpen.value = false
  onQuery()
}

function onPageChange() {
  if (suppressPageEvent) return
  syncQueryToUrl()
  loadLogs()
}

function onSizeChange() {
  resetPage()
  syncQueryToUrl()
  loadLogs()
}

// —— URL query 同步辅助 ——
function canonicalQuery(q: Record<string, unknown>): string {
  return Object.keys(q)
    .filter((k) => q[k] !== undefined && q[k] !== null && q[k] !== '')
    .sort()
    .map((k) => {
      const v = q[k]
      const val = Array.isArray(v) ? v[0] : v
      return `${k}=${String(val)}`
    })
    .join('&')
}

function qv(key: string): string {
  const v = route.query[key]
  if (Array.isArray(v)) return v[0] ?? ''
  return v ?? ''
}

/** 挂载时从 URL query 恢复筛选（page/status 转数字，stream/degraded 转布尔） */
function restoreFromQuery() {
  const from = qv('from_ts')
  const to = qv('to_ts')
  if (from && to) {
    const d1 = new Date(from)
    const d2 = new Date(to)
    if (!Number.isNaN(d1.getTime()) && !Number.isNaN(d2.getTime())) {
      range.value = [d1, d2]
    }
  }
  keyId.value = qv('key_id')
  upstreamId.value = qv('upstream_id')
  model.value = qv('model')
  requestId.value = qv('request_id')

  const statusGroupRaw = qv('status_group')
  const statusGroups = ['success', '4xx', '5xx', '429', 'degraded']
  statusGroup.value = statusGroups.includes(statusGroupRaw) ? statusGroupRaw : ''

  const status = qv('status')
  statusStr.value = /^\d+$/.test(status) ? status : ''

  const stream = qv('stream')
  streamOpt.value = stream === 'true' || stream === 'false' ? stream : ''

  const degraded = qv('degraded')
  degradedOpt.value = degraded === 'true' || degraded === 'false' ? degraded : ''

  const page = Number.parseInt(qv('page'), 10)
  if (Number.isFinite(page) && page > 0) currentPage.value = page
  const size = Number.parseInt(qv('page_size'), 10)
  if (Number.isFinite(size) && size > 0) pageSize.value = size

  advancedOpen.value = hasAdvancedFilter()
}

async function loadKeys() {
  try {
    keys.value = await keyApi.list()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

async function loadUpstreams() {
  try {
    upstreams.value = await upstreamApi.list()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

onMounted(() => {
  restoreFromQuery()
  loadKeys()
  loadUpstreams()
  loadLogs()
})

// —— 展示辅助 ——
function keyLabel(k: ApiKeyRow): string {
  return `${k.name || '(未命名)'} · ${k.prefix}`
}

function keyText(row?: LogItem | null): string {
  if (!row) return '-'
  const parts = [row.key_prefix, row.key_name].filter(Boolean)
  return parts.length ? parts.join(' · ') : '-'
}

function fmtLogTime(iso?: string | null): string {
  if (!iso) return '-'
  const d = dayjs(iso)
  return d.isValid() ? d.format('MM-DD HH:mm:ss') : '-'
}

function protoLabel(v?: string | null): string {
  if (!v) return '-'
  return PROTOCOL_IN_LABELS[v] ?? v
}

// —— 详情抽屉 ——
const detailVisible = ref(false)
const detailLoading = ref(false)
const detail = ref<LogItem | null>(null)
const detailError = ref('')
const detailRequestId = ref('')
const detailOpen = ref(['usage_raw'])

/** 响应式：≤768px 抽屉全宽、字段单列，桌面 60% / 双列 */
const drawerSize = computed(() => (isMobile.value ? '100%' : '60%'))
const descColumn = computed(() => (isMobile.value ? 1 : 2))

async function fetchDetail() {
  const rid = detailRequestId.value
  if (!rid) return
  detailLoading.value = true
  detail.value = null
  detailError.value = ''
  try {
    detail.value = await logApi.detail(rid)
  } catch (e) {
    detailError.value = errMsg(e)
  } finally {
    detailLoading.value = false
  }
}

function openDetail(row: LogItem) {
  detailVisible.value = true
  detailRequestId.value = row.request_id
  fetchDetail()
}

function retryDetail() {
  fetchDetail()
}

function copyRequestId(id: string) {
  if (!id) return
  void copyText(id, '已复制 request_id')
}

function isEmptyJson(v: unknown): boolean {
  if (v === null || v === undefined || v === '') return true
  if (Array.isArray(v)) return v.length === 0
  if (typeof v === 'object') return Object.keys(v).length === 0
  return false
}

function jsonText(v: unknown): string {
  if (isEmptyJson(v)) return ''
  try {
    return JSON.stringify(v, null, 2)
  } catch {
    return String(v)
  }
}

// —— CSV 导出 ——
const exporting = ref(false)

async function exportCsv() {
  exporting.value = true
  try {
    const blob = await logApi.exportCsv(buildFilterQuery())
    downloadBlob(blob, 'usage_logs.csv')
    ElMessage.success('已导出 usage_logs.csv')
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    exporting.value = false
  }
}

// —— 清理旧日志 ——
/** 最近一次列表加载完成时间（提示异步落库延迟） */
const lastLoadedAt = ref('')
/** 游标浏览模式（M14 §5.2 深页 keyset 分页）：与页码模式互斥 */
const cursorMode = ref(false)
const cursorRows = ref<LogItem[]>([])
const cursorNext = ref<string | null>(null)
const cursorLoading = ref(false)

const cleanupVisible = ref(false)
const cleanupBefore = ref<Date | null>(null)
const cleanupLoading = ref(false)
const cleanupExecuting = ref(false)
const cleanupPreview = ref<PartitionInfo[]>([])
const cleanupPreviewDone = ref(false)
/** 实际生效截止（分区边界对齐） */
const cleanupEffectiveBefore = ref<string | null>(null)
/** dry-run 汇总（行数 / Token / 成本） */
const cleanupSummary = ref<CleanupSummary | null>(null)
const cleanupHourlyRows = ref(0)

function openCleanup() {
  cleanupBefore.value = null
  cleanupPreview.value = []
  cleanupPreviewDone.value = false
  cleanupEffectiveBefore.value = null
  cleanupSummary.value = null
  cleanupHourlyRows.value = 0
  cleanupVisible.value = true
}

async function runCleanupPreview() {
  if (!cleanupBefore.value) {
    ElMessage.warning('请选择 before 日期')
    return
  }
  cleanupLoading.value = true
  try {
    const resp = await logApi.cleanup({ before: tsToIso(cleanupBefore.value), dry_run: true })
    cleanupPreview.value = resp.dropped
    cleanupEffectiveBefore.value = resp.effective_before ?? null
    cleanupSummary.value = resp.summary ?? null
    cleanupHourlyRows.value = resp.hourly_rows ?? 0
    cleanupPreviewDone.value = true
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    cleanupLoading.value = false
  }
}

async function confirmCleanup() {
  if (!cleanupBefore.value) return
  const rows = cleanupSummary.value?.log_rows
  const ok = await confirmDanger({
    title: '删除日志分区',
    message:
      `将删除 ${cleanupPreview.value.length} 个完整分区` +
      `${rows != null ? `（约 ${fmtInt(rows)} 行明细）` : ''}，` +
      '按分区边界整分区 DROP，明细与滚动聚合表同步清理，**数据不可恢复**；' +
      '独立配额计数器 quota_usage 不受影响。',
    confirmText: '确认删除',
  })
  if (!ok) return
  cleanupExecuting.value = true
  try {
    const resp = await logApi.cleanup({ before: tsToIso(cleanupBefore.value), dry_run: false })
    const n = resp.dropped.length
    const hourly = resp.hourly_rows ?? 0
    ElMessage.success(`已删除 ${n} 个分区 · 聚合表 ${hourly} 行`)
    cleanupVisible.value = false
    // 删除后当前页可能越界：回到第 1 页再刷新
    onQuery()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    cleanupExecuting.value = false
  }
}
// 供 /usage 页头「刷新全部」调用
defineExpose({ refresh: loadLogs })
</script>

<style scoped>

/* 首屏骨架：列宽比例贴近真实表格，避免数据到达时跳动 */
.log-skeleton {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 10px 12px;
}

.log-skeleton-row {
  display: flex;
  align-items: center;
  gap: 16px;
}

.log-skeleton-card {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px;
  border: 1px solid var(--nx-border-subtle);
  border-radius: var(--nx-radius-md);
}
.time-cell,
.model-cell {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.time-main,
.model-main {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.time-rid {
  color: var(--el-text-color-secondary);
  font-size: var(--nx-font-caption);
}
.proto-line {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex-wrap: wrap;
}
.status-sub {
  margin-top: 2px;
  line-height: 1.3;
  white-space: normal;
}

/* 移动端日志卡片 */
.log-cards {
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-height: 80px;
}
.log-card {
  border: 1px solid var(--nx-border-subtle);
  border-radius: var(--nx-radius-md);
  padding: 10px 12px;
  background: var(--el-bg-color);
}
.log-card-head,
.log-card-meta,
.log-card-foot {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}
.log-card-head {
  margin-bottom: 4px;
}
.log-card-model {
  font-weight: 600;
  color: var(--el-text-color-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.log-card-cost {
  color: var(--el-text-color-regular);
  font-size: var(--nx-font-caption);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
.log-card-meta {
  flex-direction: column;
  align-items: flex-start;
  gap: 2px;
  font-size: var(--nx-font-caption);
}
.log-card-meta-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 4px 10px;
}
.log-card-foot {
  margin-top: 6px;
  font-size: var(--nx-font-caption);
}

.table-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 12px;
  font-weight: 600;
  color: var(--el-text-color-primary);
}

.convert-tag {
  cursor: default;
}

.token-cell {
  display: flex;
  gap: 8px;
  font-size: var(--nx-font-caption);
  color: var(--el-text-color-primary);
}

.cost-cell {
  font-size: var(--nx-font-caption);
  line-height: 1.5;
  color: var(--el-text-color-primary);
}


.json-cards {
  margin-top: 16px;
}

/* 详情抽屉：request_id 顶栏 + 分组标题 */
.detail-head {
  display: flex;
  align-items: center;
  gap: 8px;
  padding-bottom: 10px;
  margin-bottom: 4px;
  border-bottom: 1px solid var(--nx-border-subtle);
  flex-wrap: wrap;
}
.detail-head .rid {
  font-size: var(--nx-font-caption);
  color: var(--el-text-color-regular);
  word-break: break-all;
}
.status-cell {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.status-cell .has-error {
  cursor: help;
}
.group-title {
  margin: 16px 0 8px;
  font-size: var(--nx-font-small);
  font-weight: 600;
  color: var(--el-text-color-primary);
}
.help-icon {
  flex-shrink: 0;
  margin-left: var(--nx-space-1);
  color: var(--nx-text-3);
  font-size: var(--nx-font-small);
  vertical-align: -2px;
  cursor: help;
}

/* JSON 详情块：抽屉内自滚动，避免内外两层滚动条叠加 */
.json-cards pre {
  margin: 0;
  max-height: 320px;
  overflow: auto;
}

/* 详情错误块（区别于「无数据」空态；列表错误态用全局样式） */
.error-block {
  margin-top: 14px;
  padding: 10px 12px;
  background: var(--el-color-danger-light-9);
  color: var(--el-color-danger);
  border-radius: var(--nx-radius);
  font-family: 'SFMono-Regular', Consolas, Menlo, monospace;
  word-break: break-all;
}

.cleanup-summary {
  margin-bottom: 10px;
}
.cleanup-desc {
  margin-bottom: 14px;
  padding: 10px 12px;
  background: var(--el-fill-color-light);
  border-radius: var(--nx-radius);
}

/* 日志行/卡片整行进入详情；内部按钮阻止冒泡 */
.log-table :deep(.el-table__row) {
  cursor: pointer;
}
.row-error {
  display: block;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--nx-danger);
  font-size: var(--nx-font-caption);
  line-height: 1.5;
}
.log-card {
  cursor: pointer;
}
.log-card:focus-visible {
  outline: 2px solid var(--nx-focus-ring);
  outline-offset: 2px;
}
.log-card-error {
  margin-top: 6px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--nx-danger);
  font-size: var(--nx-font-caption);
}
.adv-count {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 16px;
  height: 16px;
  margin-left: 4px;
  padding: 0 4px;
  border-radius: var(--nx-radius-round);
  background: var(--nx-accent-soft);
  color: var(--nx-accent);
  font-size: 11px;
  font-weight: 600;
  line-height: 1;
}

/* 数字列右对齐（对齐 M20 密度规范） */
.token-cell,
.cost-cell {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 1px;
  font-variant-numeric: tabular-nums;
}
.cost-cell > div {
  white-space: nowrap;
}

/* 表尾：分页与游标区与表格用细分隔线隔开（与统计段表尾一致） */
.pagination,
.cursor-foot {
  margin-top: var(--nx-space-3);
  padding-top: var(--nx-space-3);
  border-top: 1px solid var(--nx-border-subtle);
}

</style>
