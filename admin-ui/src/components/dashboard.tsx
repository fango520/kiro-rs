import { useState } from 'react'
import { Activity, FileUp, KeyRound, LayoutDashboard, ListChecks, LogOut, Moon, Plus, RefreshCw, RotateCw, Server, Sun, Trash2, Upload, Wallet } from 'lucide-react'
import { useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { storage } from '@/lib/storage'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Checkbox } from '@/components/ui/checkbox'
import { BalanceDialog } from '@/components/balance-dialog'
import { AddCredentialDialog } from '@/components/add-credential-dialog'
import { BatchImportDialog } from '@/components/batch-import-dialog'
import { KamImportDialog } from '@/components/kam-import-dialog'
import { useApiKeys, useCreateApiKey, useCredentials, useDeleteApiKey, useDeleteCredential, useLoadBalancingMode, useRequestLogs, useSetApiKeyEnabled, useSetDisabled, useSetLoadBalancingMode } from '@/hooks/use-credentials'
import { forceRefreshToken, getCredentialBalance } from '@/api/credentials'
import { extractErrorMessage } from '@/lib/utils'
import type { ApiKeyView, BalanceResponse, CredentialStatusItem, RequestLogEntry } from '@/types/api'

interface DashboardProps { onLogout: () => void }
type Tab = 'overview' | 'credentials' | 'keys' | 'logs'

const fmt = (n?: number | null) => new Intl.NumberFormat('zh-CN').format(n || 0)
function timeAgo(value?: string | null) {
  if (!value) return '从未'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  const s = Math.max(0, Math.floor((Date.now() - date.getTime()) / 1000))
  if (s < 60) return `${s} 秒前`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m} 分钟前`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h} 小时前`
  return `${Math.floor(h / 24)} 天前`
}
function authLabel(c: CredentialStatusItem) {
  if (c.authMethod === 'api_key') return 'API Key'
  if (c.authMethod === 'idc') return 'IdC'
  if (c.authMethod === 'social') return 'Social'
  return c.authMethod || '-'
}
function NavButton({ tab, active, icon: Icon, label, onClick }: { tab: Tab; active: Tab; icon: typeof LayoutDashboard; label: string; onClick: (tab: Tab) => void }) {
  return <Button variant={active === tab ? 'secondary' : 'ghost'} size="sm" onClick={() => onClick(tab)}><Icon className="h-4 w-4 mr-2" />{label}</Button>
}

export function Dashboard({ onLogout }: DashboardProps) {
  const [tab, setTab] = useState<Tab>('overview')
  const [selected, setSelected] = useState<Set<number>>(new Set())
  const [balanceId, setBalanceId] = useState<number | null>(null)
  const [balanceOpen, setBalanceOpen] = useState(false)
  const [addOpen, setAddOpen] = useState(false)
  const [batchOpen, setBatchOpen] = useState(false)
  const [kamOpen, setKamOpen] = useState(false)
  const [balanceMap, setBalanceMap] = useState<Map<number, BalanceResponse>>(new Map())
  const [loadingBalance, setLoadingBalance] = useState<Set<number>>(new Set())
  const [keyName, setKeyName] = useState('')
  const [newKey, setNewKey] = useState<string | null>(null)
  const [dark, setDark] = useState(() => typeof window !== 'undefined' && document.documentElement.classList.contains('dark'))

  const queryClient = useQueryClient()
  const { data, isLoading, error, refetch } = useCredentials()
  const { data: apiKeys } = useApiKeys()
  const { data: logsData } = useRequestLogs()
  const setDisabled = useSetDisabled()
  const deleteCredential = useDeleteCredential()
  const createApiKey = useCreateApiKey()
  const setApiKeyEnabled = useSetApiKeyEnabled()
  const deleteApiKey = useDeleteApiKey()
  const { data: mode, isLoading: modeLoading } = useLoadBalancingMode()
  const { mutate: setMode, isPending: settingMode } = useSetLoadBalancingMode()

  const credentials = data?.credentials || []
  const logs = logsData?.logs || []
  const summary = logsData?.summary
  const disabledCount = credentials.filter(c => c.disabled).length
  const selectedDisabledCount = [...selected].filter(id => credentials.find(c => c.id === id)?.disabled).length

  const refreshAll = () => {
    refetch(); queryClient.invalidateQueries({ queryKey: ['apiKeys'] }); queryClient.invalidateQueries({ queryKey: ['requestLogs'] }); toast.success('已刷新')
  }
  const logout = () => { storage.removeApiKey(); queryClient.clear(); onLogout() }
  const toggleDark = () => { setDark(!dark); document.documentElement.classList.toggle('dark') }
  const toggleMode = () => {
    const next = (mode?.mode || 'priority') === 'priority' ? 'balanced' : 'priority'
    setMode(next, { onSuccess: () => toast.success(`已切换到${next === 'priority' ? '优先级模式' : '均衡负载模式'}`), onError: err => toast.error(extractErrorMessage(err)) })
  }
  const toggleSelect = (id: number) => setSelected(prev => { const next = new Set(prev); next.has(id) ? next.delete(id) : next.add(id); return next })
  const openBalance = (id: number) => { setBalanceId(id); setBalanceOpen(true) }
  const queryBalance = async (id: number) => {
    setLoadingBalance(prev => new Set(prev).add(id))
    try { const b = await getCredentialBalance(id); setBalanceMap(prev => new Map(prev).set(id, b)); toast.success(`凭据 #${id} 已更新`) }
    catch (err) { toast.error(extractErrorMessage(err)) }
    finally { setLoadingBalance(prev => { const next = new Set(prev); next.delete(id); return next }) }
  }
  const refreshToken = async (id: number) => {
    try { await forceRefreshToken(id); toast.success(`凭据 #${id} Token 已刷新`); queryClient.invalidateQueries({ queryKey: ['credentials'] }) }
    catch (err) { toast.error(extractErrorMessage(err)) }
  }
  const removeCredential = (c: CredentialStatusItem) => {
    if (!c.disabled) { toast.error('请先禁用凭据再删除'); return }
    if (!confirm(`确定删除 ${c.email || `凭据 #${c.id}`} 吗？`)) return
    deleteCredential.mutate(c.id, { onSuccess: res => toast.success(res.message), onError: err => toast.error(extractErrorMessage(err)) })
  }
  const clearDisabled = async () => {
    const disabled = credentials.filter(c => c.disabled)
    if (!disabled.length) { toast.error('没有已禁用凭据'); return }
    if (!confirm(`确定清除 ${disabled.length} 个已禁用凭据吗？`)) return
    for (const c of disabled) await new Promise<void>(resolve => deleteCredential.mutate(c.id, { onSettled: () => resolve() }))
    setSelected(new Set()); toast.success('已清除已禁用凭据')
  }
  const batchDelete = async () => {
    const ids = [...selected].filter(id => credentials.find(c => c.id === id)?.disabled)
    if (!ids.length) { toast.error('只能删除已禁用凭据'); return }
    if (!confirm(`确定删除 ${ids.length} 个已禁用凭据吗？`)) return
    for (const id of ids) await new Promise<void>(resolve => deleteCredential.mutate(id, { onSettled: () => resolve() }))
    setSelected(new Set()); toast.success('批量删除完成')
  }
  const createKey = () => {
    const name = keyName.trim()
    if (!name) { toast.error('请输入 Key 名称'); return }
    createApiKey.mutate(name, { onSuccess: res => { setNewKey(res.key); setKeyName(''); toast.success('API Key 已创建') }, onError: err => toast.error(extractErrorMessage(err)) })
  }
  const copyKey = async (value: string) => { await navigator.clipboard.writeText(value); toast.success('已复制') }

  if (isLoading) return <div className="min-h-screen flex items-center justify-center bg-background"><div className="animate-spin rounded-full h-12 w-12 border-b-2 border-primary" /></div>
  if (error) return <div className="min-h-screen flex items-center justify-center bg-background p-4"><Card className="w-full max-w-md"><CardContent className="pt-6 text-center"><div className="text-red-500 mb-4">加载失败</div><p className="text-muted-foreground mb-4">{(error as Error).message}</p><Button onClick={() => refetch()}>重试</Button></CardContent></Card></div>

  return <div className="min-h-screen bg-background">
    <header className="sticky top-0 z-50 w-full border-b bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/60">
      <div className="container flex h-14 items-center justify-between px-4 md:px-8">
        <div className="flex items-center gap-2"><Server className="h-5 w-5" /><span className="font-semibold">Kiro Admin</span></div>
        <div className="flex items-center gap-1">
          <NavButton tab="overview" active={tab} icon={LayoutDashboard} label="仪表盘" onClick={setTab} />
          <NavButton tab="credentials" active={tab} icon={ListChecks} label="凭据" onClick={setTab} />
          <NavButton tab="keys" active={tab} icon={KeyRound} label="Keys" onClick={setTab} />
          <NavButton tab="logs" active={tab} icon={Activity} label="日志" onClick={setTab} />
          <Button variant="outline" size="sm" onClick={toggleMode} disabled={modeLoading || settingMode}>{mode?.mode === 'priority' ? '优先级' : '均衡负载'}</Button>
          <Button variant="ghost" size="icon" onClick={toggleDark}>{dark ? <Sun className="h-5 w-5" /> : <Moon className="h-5 w-5" />}</Button>
          <Button variant="ghost" size="icon" onClick={refreshAll}><RefreshCw className="h-5 w-5" /></Button>
          <Button variant="ghost" size="icon" onClick={logout}><LogOut className="h-5 w-5" /></Button>
        </div>
      </div>
    </header>
    <main className="container mx-auto px-4 md:px-8 py-6">
      {tab === 'overview' && <Overview total={data?.total || 0} available={data?.available || 0} disabled={disabledCount} currentId={data?.currentId || 0} keys={apiKeys?.length || 0} requests={summary?.requestCount || 0} success={summary?.successCount || 0} errors={summary?.errorCount || 0} input={summary?.inputTokens || 0} output={summary?.outputTokens || 0} />}
      {tab === 'credentials' && <CredentialsPanel credentials={credentials} selected={selected} selectedDisabledCount={selectedDisabledCount} disabledCount={disabledCount} balanceMap={balanceMap} loadingBalance={loadingBalance} onSelectAll={() => setSelected(new Set(credentials.map(c => c.id)))} onClearSelection={() => setSelected(new Set())} onToggleSelect={toggleSelect} onToggleCredential={c => setDisabled.mutate({ id: c.id, disabled: !c.disabled })} onBalance={openBalance} onQuery={queryBalance} onRefreshToken={refreshToken} onDelete={removeCredential} onBatchDelete={batchDelete} onClearDisabled={clearDisabled} onAdd={() => setAddOpen(true)} onBatch={() => setBatchOpen(true)} onKam={() => setKamOpen(true)} />}
      {tab === 'keys' && <KeysPanel apiKeys={apiKeys || []} keyName={keyName} newKey={newKey} creating={createApiKey.isPending} onNameChange={setKeyName} onCreate={createKey} onCopy={copyKey} onToggle={(id, enabled) => setApiKeyEnabled.mutate({ id, enabled })} onDelete={id => deleteApiKey.mutate(id)} />}
      {tab === 'logs' && <LogsPanel logs={logs} />}
    </main>
    <BalanceDialog credentialId={balanceId} open={balanceOpen} onOpenChange={setBalanceOpen} />
    <AddCredentialDialog open={addOpen} onOpenChange={setAddOpen} />
    <BatchImportDialog open={batchOpen} onOpenChange={setBatchOpen} />
    <KamImportDialog open={kamOpen} onOpenChange={setKamOpen} />
  </div>
}

function Overview(p: { total: number; available: number; disabled: number; currentId: number; keys: number; requests: number; success: number; errors: number; input: number; output: number }) {
  const cards = [['凭据总数', p.total], ['可用凭据', p.available], ['已禁用', p.disabled], ['当前活跃', p.currentId ? `#${p.currentId}` : '-'], ['API Keys', p.keys], ['请求数', p.requests], ['成功 / 失败', `${fmt(p.success)} / ${fmt(p.errors)}`], ['总 Tokens', fmt(p.input + p.output)]]
  return <div className="space-y-6"><div className="grid gap-4 md:grid-cols-4">{cards.map(([title, value]) => <Card key={title}><CardHeader className="pb-2"><CardTitle className="text-sm text-muted-foreground">{title}</CardTitle></CardHeader><CardContent><div className="text-2xl font-bold">{value}</div></CardContent></Card>)}</div><div className="grid gap-4 md:grid-cols-2"><Card><CardHeader><CardTitle className="text-base">Token 汇总</CardTitle></CardHeader><CardContent className="grid grid-cols-3 gap-4"><Metric label="输入" value={fmt(p.input)} /><Metric label="输出" value={fmt(p.output)} /><Metric label="合计" value={fmt(p.input + p.output)} /></CardContent></Card><Card><CardHeader><CardTitle className="text-base">请求健康度</CardTitle></CardHeader><CardContent className="grid grid-cols-3 gap-4"><Metric label="请求" value={fmt(p.requests)} /><Metric label="成功" value={fmt(p.success)} /><Metric label="失败" value={fmt(p.errors)} /></CardContent></Card></div></div>
}
function Metric({ label, value }: { label: string; value: string }) { return <div><div className="text-xs text-muted-foreground">{label}</div><div className="text-xl font-semibold mt-1">{value}</div></div> }
function CredentialsPanel(p: { credentials: CredentialStatusItem[]; selected: Set<number>; selectedDisabledCount: number; disabledCount: number; balanceMap: Map<number, BalanceResponse>; loadingBalance: Set<number>; onSelectAll: () => void; onClearSelection: () => void; onToggleSelect: (id: number) => void; onToggleCredential: (c: CredentialStatusItem) => void; onBalance: (id: number) => void; onQuery: (id: number) => void; onRefreshToken: (id: number) => void; onDelete: (c: CredentialStatusItem) => void; onBatchDelete: () => void; onClearDisabled: () => void; onAdd: () => void; onBatch: () => void; onKam: () => void }) {
  return <Card><CardHeader><div className="flex flex-wrap items-center justify-between gap-3"><CardTitle>凭据管理</CardTitle><div className="flex flex-wrap gap-2"><Button size="sm" variant="outline" onClick={p.onSelectAll}>全选</Button><Button size="sm" variant="outline" onClick={p.onClearSelection}>取消</Button><Button size="sm" variant="destructive" disabled={p.selectedDisabledCount === 0} onClick={p.onBatchDelete}><Trash2 className="h-4 w-4 mr-2" />批量删除</Button><Button size="sm" variant="outline" disabled={p.disabledCount === 0} onClick={p.onClearDisabled}><Trash2 className="h-4 w-4 mr-2" />清除禁用</Button><Button size="sm" variant="outline" onClick={p.onKam}><FileUp className="h-4 w-4 mr-2" />KAM 导入</Button><Button size="sm" variant="outline" onClick={p.onBatch}><Upload className="h-4 w-4 mr-2" />批量导入</Button><Button size="sm" onClick={p.onAdd}><Plus className="h-4 w-4 mr-2" />添加</Button></div></div></CardHeader><CardContent><div className="overflow-auto rounded-md border"><table className="w-full text-sm"><thead className="bg-muted/50"><tr><th className="p-3 text-left w-10"></th><th className="p-3 text-left">账号</th><th className="p-3 text-left">状态</th><th className="p-3 text-left">认证</th><th className="p-3 text-left">优先级</th><th className="p-3 text-left">订阅/额度</th><th className="p-3 text-left">调用</th><th className="p-3 text-left">最后调用</th><th className="p-3 text-right">操作</th></tr></thead><tbody>{p.credentials.map(c => { const b = p.balanceMap.get(c.id); return <tr key={c.id} className="border-t"><td className="p-3"><Checkbox checked={p.selected.has(c.id)} onCheckedChange={() => p.onToggleSelect(c.id)} /></td><td className="p-3"><div className="font-medium">{c.email || `凭据 #${c.id}`}</div><div className="text-xs text-muted-foreground">#{c.id} · {c.endpoint}</div></td><td className="p-3"><div className="flex items-center gap-2"><Switch checked={!c.disabled} onCheckedChange={() => p.onToggleCredential(c)} /><Badge variant={c.disabled ? 'destructive' : 'success'}>{c.disabled ? '禁用' : '启用'}</Badge>{c.isCurrent && <Badge variant="outline">当前</Badge>}</div>{c.disabledReason && <div className="text-xs text-red-500 mt-1">{c.disabledReason}</div>}</td><td className="p-3"><Badge variant="secondary">{authLabel(c)}</Badge>{c.provider && <div className="text-xs text-muted-foreground mt-1">{c.provider}</div>}</td><td className="p-3">{c.priority}</td><td className="p-3"><div>{p.loadingBalance.has(c.id) ? '查询中...' : b?.subscriptionTitle || '未知'}</div>{b && <div className="text-xs text-muted-foreground">{b.currentUsage}/{b.usageLimit}</div>}</td><td className="p-3">{fmt(c.successCount)}{c.failureCount > 0 && <div className="text-xs text-red-500">失败 {c.failureCount}</div>}</td><td className="p-3">{timeAgo(c.lastUsedAt)}</td><td className="p-3"><div className="flex justify-end gap-1"><Button size="icon" variant="ghost" title="余额" onClick={() => p.onBalance(c.id)}><Wallet className="h-4 w-4" /></Button><Button size="icon" variant="ghost" title="查询" onClick={() => p.onQuery(c.id)}><RefreshCw className="h-4 w-4" /></Button><Button size="icon" variant="ghost" title="刷新 Token" onClick={() => p.onRefreshToken(c.id)}><RotateCw className="h-4 w-4" /></Button><Button size="icon" variant="ghost" title="删除" onClick={() => p.onDelete(c)}><Trash2 className="h-4 w-4" /></Button></div></td></tr> })}</tbody></table>{p.credentials.length === 0 && <div className="p-8 text-center text-muted-foreground">暂无凭据</div>}</div></CardContent></Card>
}

function KeysPanel(p: { apiKeys: ApiKeyView[]; keyName: string; newKey: string | null; creating: boolean; onNameChange: (v: string) => void; onCreate: () => void; onCopy: (v: string) => void; onToggle: (id: string, enabled: boolean) => void; onDelete: (id: string) => void }) {
  return <Card><CardHeader><CardTitle>API Key 管理</CardTitle></CardHeader><CardContent className="space-y-4"><div className="flex gap-2 max-w-xl"><Input value={p.keyName} onChange={e => p.onNameChange(e.target.value)} placeholder="Key 名称，例如 sub2api-main" /><Button onClick={p.onCreate} disabled={p.creating}><Plus className="h-4 w-4 mr-2" />新增 Key</Button></div>{p.newKey && <div className="rounded-md border p-3 bg-muted/40 text-sm"><div className="font-medium mb-2">新 Key 只显示一次</div><div className="flex gap-2"><code className="flex-1 break-all">{p.newKey}</code><Button size="sm" variant="outline" onClick={() => p.onCopy(p.newKey!)}>复制</Button></div></div>}<div className="overflow-auto rounded-md border"><table className="w-full text-sm"><thead className="bg-muted/50"><tr><th className="p-3 text-left">名称</th><th className="p-3 text-left">Key</th><th className="p-3 text-left">状态</th><th className="p-3 text-left">请求</th><th className="p-3 text-left">Tokens</th><th className="p-3 text-left">最后使用</th><th className="p-3 text-right">操作</th></tr></thead><tbody>{p.apiKeys.map(k => <tr key={k.id} className="border-t"><td className="p-3 font-medium">{k.name}</td><td className="p-3 font-mono text-xs">{k.keyPrefix}</td><td className="p-3"><div className="flex items-center gap-2"><Switch checked={k.enabled} onCheckedChange={enabled => p.onToggle(k.id, enabled)} /><Badge variant={k.enabled ? 'success' : 'secondary'}>{k.enabled ? '启用' : '停用'}</Badge></div></td><td className="p-3">{fmt(k.requestCount)}</td><td className="p-3">{fmt(k.inputTokens + k.outputTokens)}<div className="text-xs text-muted-foreground">in {fmt(k.inputTokens)} / out {fmt(k.outputTokens)}</div></td><td className="p-3">{timeAgo(k.lastUsedAt)}</td><td className="p-3 text-right"><Button size="icon" variant="ghost" disabled={k.id === 'default'} onClick={() => p.onDelete(k.id)}><Trash2 className="h-4 w-4" /></Button></td></tr>)}</tbody></table></div></CardContent></Card>
}

function LogsPanel({ logs }: { logs: RequestLogEntry[] }) {
  return <Card><CardHeader><CardTitle>请求日志</CardTitle></CardHeader><CardContent><div className="overflow-auto rounded-md border"><table className="w-full text-sm"><thead className="bg-muted/50"><tr><th className="p-3 text-left">时间</th><th className="p-3 text-left">Key</th><th className="p-3 text-left">模型</th><th className="p-3 text-left">状态</th><th className="p-3 text-left">模式</th><th className="p-3 text-left">凭据</th><th className="p-3 text-left">Tokens</th><th className="p-3 text-left">耗时</th><th className="p-3 text-left">错误</th></tr></thead><tbody>{logs.map(log => <tr key={log.id} className="border-t"><td className="p-3 whitespace-nowrap">{timeAgo(log.timestamp)}</td><td className="p-3"><div>{log.apiKeyName}</div><div className="text-xs text-muted-foreground font-mono">{log.apiKeyPrefix}</div></td><td className="p-3">{log.model}</td><td className="p-3"><Badge variant={log.success ? 'success' : 'destructive'}>{log.status}</Badge></td><td className="p-3">{log.stream ? 'stream' : 'json'}</td><td className="p-3">{log.credentialId ? `#${log.credentialId}` : '-'}</td><td className="p-3">{fmt(log.totalTokens)}<div className="text-xs text-muted-foreground">in {fmt(log.inputTokens)} / out {fmt(log.outputTokens)}</div></td><td className="p-3">{log.durationMs}ms</td><td className="p-3 max-w-sm truncate text-red-500">{log.error || '-'}</td></tr>)}</tbody></table>{logs.length === 0 && <div className="p-8 text-center text-muted-foreground">暂无请求日志</div>}</div></CardContent></Card>
}
