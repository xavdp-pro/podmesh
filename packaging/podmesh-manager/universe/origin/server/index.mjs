import { createApp } from './app.mjs'
import { factsReader, markReader, observer, readConfig } from './resident.mjs'

const CONFIG = process.env.PODMESH_MANAGER_CONFIG || '/etc/podmesh-manager/config.json'
const STATE = process.env.PODMESH_MANAGER_STATE || '/var/lib/podmesh-manager'
const BINARY = process.env.PODMESH_MANAGER_BINARY || '/usr/lib/podmesh-manager/podmesh-managerd'
// The active manager's mark, written by PodMesh at the exclusive publication and removed at the
// withdrawal or the fence. Its path was renamed on 2026-09-17: until every PodMesh node writes the
// new path, the origin also reads the previous one, and a node that writes the new path removes both.
const MARKS = process.env.PODMESH_ACTIVE_MANAGER_MARK
  ? [process.env.PODMESH_ACTIVE_MANAGER_MARK]
  : ['/run/podmesh-manager/active-manager.json', '/run/podmesh-manager/governor.json']
const PORT = Number(process.env.PODMESH_ORIGIN_PORT || 8080)

const { identity, scope, control } = readConfig(CONFIG)
const facts = factsReader({ binary: BINARY, config: CONFIG, state: STATE })
const app = createApp({
  identity, scope,
  activeManager: markReader(MARKS),
  facts,
  append: observer({ control, scope, facts }),
  secure: process.env.PODMESH_ORIGIN_INSECURE_COOKIE !== '1',
})
app.listen(PORT, '0.0.0.0', () => console.log(`manager origin listening on ${PORT} (fail-closed until PodMesh marks this replica as the active manager)`))
