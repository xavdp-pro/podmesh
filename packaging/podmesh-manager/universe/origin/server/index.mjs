import { createApp } from './app.mjs'
import { factsReader, markReader, observer, readConfig } from './resident.mjs'

const CONFIG = process.env.PODMESH_MANAGER_CONFIG || '/etc/podmesh-manager/config.json'
const STATE = process.env.PODMESH_MANAGER_STATE || '/var/lib/podmesh-manager'
const BINARY = process.env.PODMESH_MANAGER_BINARY || '/usr/lib/podmesh-manager/podmesh-managerd'
const MARK = process.env.PODMESH_GOVERNOR_MARK || '/run/podmesh-manager/governor.json'
const PORT = Number(process.env.PODMESH_ORIGIN_PORT || 8080)

const { identity, scope, control } = readConfig(CONFIG)
const facts = factsReader({ binary: BINARY, config: CONFIG, state: STATE })
const app = createApp({
  identity, scope,
  governor: markReader(MARK),
  facts,
  append: observer({ control, scope, facts }),
  secure: process.env.PODMESH_ORIGIN_INSECURE_COOKIE !== '1',
})
app.listen(PORT, '0.0.0.0', () => console.log(`manager origin listening on ${PORT} (fail-closed until PodMesh marks this replica governor)`))
