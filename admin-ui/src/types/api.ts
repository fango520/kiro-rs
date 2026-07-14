// 凭据状态响应
export interface CredentialsStatusResponse {
  total: number
  available: number
  currentId: number
  credentials: CredentialStatusItem[]
}

// 单个凭据状态
export interface CredentialStatusItem {
  id: number
  priority: number
  disabled: boolean
  failureCount: number
  isCurrent: boolean
  expiresAt: string | null
  authMethod: string | null
  provider?: string | null
  hasProfileArn: boolean
  email?: string
  refreshTokenHash?: string
  apiKeyHash?: string
  maskedApiKey?: string
  successCount: number
  lastUsedAt: string | null
  hasProxy: boolean
  proxyUrl?: string
  refreshFailureCount: number
  disabledReason?: string
  endpoint: string
}

// 余额响应
export interface BalanceResponse {
  id: number
  subscriptionTitle: string | null
  currentUsage: number
  usageLimit: number
  remaining: number
  usagePercentage: number
  nextResetAt: number | null
}

// 成功响应
export interface SuccessResponse {
  success: boolean
  message: string
}

// 错误响应
export interface AdminErrorResponse {
  error: {
    type: string
    message: string
  }
}

// 请求类型
export interface SetDisabledRequest {
  disabled: boolean
}

export interface SetPriorityRequest {
  priority: number
}

// 添加凭据请求
export interface AddCredentialRequest {
  refreshToken?: string
  authMethod?: 'social' | 'idc' | 'api_key'
  provider?: string
  startUrl?: string
  profileArn?: string
  clientId?: string
  clientSecret?: string
  priority?: number
  authRegion?: string
  apiRegion?: string
  machineId?: string
  proxyUrl?: string
  proxyUsername?: string
  proxyPassword?: string
  kiroApiKey?: string
  endpoint?: string
}

// 添加凭据响应
export interface AddCredentialResponse {
  success: boolean
  message: string
  credentialId: number
  email?: string
}


export interface ApiKeyView {
  id: string
  name: string
  keyPrefix: string
  enabled: boolean
  createdAt: string
  lastUsedAt: string | null
  requestCount: number
  inputTokens: number
  outputTokens: number
}

export interface CreatedApiKey extends ApiKeyView {
  key: string
}

export interface CreateApiKeyRequest {
  name: string
}

export interface RequestLogEntry {
  id: string
  timestamp: string
  stage: string
  apiKeyId: string
  apiKeyName: string
  apiKeyPrefix: string
  model: string
  stream: boolean
  status: number
  success: boolean
  credentialId: number | null
  inputTokens: number
  outputTokens: number
  totalTokens: number
  durationMs: number
  error?: string | null
  details: RequestLogDetails
}

export interface RequestLogDetails {
  method?: string | null
  path?: string | null
  requestBody?: string | null
  responseBody?: string | null
  upstreamUrl?: string | null
  upstreamMethod?: string | null
  upstreamStatus?: number | null
  upstreamRequestBody?: string | null
  upstreamResponseBody?: string | null
}

export interface RequestLogSummary {
  requestCount: number
  successCount: number
  errorCount: number
  inputTokens: number
  outputTokens: number
  totalTokens: number
}

export interface RequestLogListResponse {
  logs: RequestLogEntry[]
  summary: RequestLogSummary
}
