import axios from 'axios'

// Every call is JSON to the manager's API, under /admin/api, with the HttpOnly session cookie.
// A 401 outside sign-in is a session that ended: the page goes back to sign-in saying why.
const api = axios.create({ baseURL: '/admin/api', timeout: 20000, withCredentials: true, headers: { 'Content-Type': 'application/json' } })

let onExpired = () => {}
export const setExpiredHandler = fn => { onExpired = fn }

api.interceptors.response.use(
  res => res.data,
  err => {
    const status = err.response?.status
    if (status === 401 && !String(err.config?.url || '').endsWith('/login')) onExpired()
    if (status === 503 && err.response?.data?.reason === 'not the governor') return Promise.reject({ error: 'not the governor', closed: true })
    return Promise.reject(err.response?.data || { error: err.message, unreachable: true })
  },
)
export default api
