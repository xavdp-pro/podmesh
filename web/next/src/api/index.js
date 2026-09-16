import axios from 'axios'

// Every call is JSON to the console's API under /api. The session token comes from GET /api/session
// and travels in the X-Podmesh-Token header; a 401 is a session that ended (the console restarted,
// for instance): the token is renewed once from /api/session and the same request is sent again --
// a 401 is refused before anything runs, so nothing happens twice. Mutations carry the page's own
// Origin, which the browser adds by itself; the server refuses any other.
const api = axios.create({ baseURL: '/api', timeout: 30000, headers: { 'Content-Type': 'application/json' } })

let token = null
let renew = async () => null
export const setToken = next => { token = next }
export const getToken = () => token
export const setRenewHandler = fn => { renew = fn }

api.interceptors.request.use(config => {
  if (token) config.headers['X-Podmesh-Token'] = token
  return config
})

api.interceptors.response.use(
  res => res.data,
  async err => {
    const status = err.response?.status
    const config = err.config || {}
    if (status === 401 && !config._renewed && !String(config.url || '').endsWith('/session')) {
      const next = await renew()
      if (next) { config._renewed = true; return api.request(config) }
    }
    if (err.response?.data && typeof err.response.data === 'object') return Promise.reject({ status, ...err.response.data })
    return Promise.reject({ status, error: err.message, unreachable: !err.response })
  },
)
export default api
