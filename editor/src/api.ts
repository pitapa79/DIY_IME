import type { CoreSession, Definition, Graph, Report, RuntimeInput, SessionResult, SessionState } from './model';

export class ApiError extends Error {
  constructor(public code: string, message: string, public state?: SessionState) { super(message); }
}
async function request<T>(path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
  const response = await fetch(path, {
    method: body !== undefined ? 'POST' : 'GET',
    headers: body !== undefined ? { 'Content-Type': 'application/json' } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined, signal,
  });
  const result = await response.json();
  if (!response.ok || !result.ok) throw new ApiError(result.error?.code ?? 'bridge-error', result.error?.message ?? 'Rust コアと通信できませんでした。', result.state);
  return result.data as T;
}
export const api = {
  definitions: () => request<Definition[]>('/api/nodes'),
  sample: (name: 'official-dictionary' | 'dictionary-exact' | 'weather' | 'weather-negative' | 'hello' | 'branch' | 'runtime-input' | 'runtime-romaji' | 'candidate-selection' | 'practical-pipeline' | 'stringify') => request<Graph>(`/api/examples/${name}`),
  validate: (graph: Graph, signal?: AbortSignal) => request<{ valid: true; order: string[] }>('/api/graph/validate', graph, signal),
  run: (graph: Graph, signal?: AbortSignal) => request<Report>('/api/graph/run', graph, signal),
  openSession: () => request<CoreSession>('/api/sessions', {}),
  closeSession: (id: string) => fetch(`/api/sessions/${id}`, { method: 'DELETE', keepalive: true }).then(() => {}).catch(() => {}),
  sessionStatus: (id: string) => request<SessionResult>(`/api/sessions/${id}`),
  sessionValidate: (id: string, graph: Graph) => request<SessionResult<{ valid: true; order: string[] }>>(`/api/sessions/${id}/validate`, { graph }),
  prepare: (id: string, graph: Graph) => request<SessionResult>(`/api/sessions/${id}/prepare`, { graph }),
  activate: (id: string, token: string) => request<SessionResult>(`/api/sessions/${id}/activate`, { token }),
  runActive: (id: string, version: number, input?: RuntimeInput) => request<SessionResult<Report>>(`/api/sessions/${id}/run`, { expectedPlanVersion: version, ...(input ? { input } : {}) }),
  rollback: (id: string, version: number) => request<SessionResult>(`/api/sessions/${id}/rollback`, { expectedPlanVersion: version }),
};
