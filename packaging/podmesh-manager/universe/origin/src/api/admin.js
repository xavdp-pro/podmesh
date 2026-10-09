import api from './index'

export const getState = () => api.get('/state')
export const login = (login, password) => api.post('/login', { login, password })
export const logout = () => api.post('/logout', {})
export const changePassword = (current, next, again) => api.post('/password', { current, next, again })
export const createAdministrator = (login, password) => api.post('/users', { login, password })
export const revokeAdministrator = login => api.post('/revoke', { login })
