import api from './index'

// The console server's routes, one function each. The front adds no transport logic: the server
// reaches the hosts and runs the workstation tools.
export const getSession = () => api.get('/session')
export const getSnapshot = () => api.get('/snapshot', { timeout: 60000 })
export const getHealth = () => api.get('/health', { timeout: 120000 })
export const getDetails = (hostId, containerPath) => api.post(`/hosts/${hostId}/details`, { container_path: containerPath }, { timeout: 80000 })
export const postAction = (hostId, body) => api.post(`/hosts/${hostId}/actions`, body, { timeout: 330000 })
export const postOperation = (hostId, body) => api.post(`/hosts/${hostId}/operations`, body, { timeout: 330000 })
export const getReplication = (hostId, universe) => api.get(`/replication/${hostId}/${universe}`, { timeout: 120000 })
export const postReplication = body => api.post('/replication', body, { timeout: 620000 })
export const postMove = body => api.post('/moves', body, { timeout: 620000 })
