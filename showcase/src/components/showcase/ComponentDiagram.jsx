import { useId } from 'react'

const COPY = {
  fr: {
    heading: 'Le rôle de chaque élément',
    podmesh: { title: 'PodMesh coordonne les applications entre hôtes', nodes: [['Serveur A', 'Application', 'Services et données'], ['PodMesh', 'Déplacement', 'ou réplication'], ['Serveur B', 'Application transférée', 'ou réplique']], caption: 'PodMesh coordonne le passage de l’application et de ses données d’un hôte à l’autre.' },
    meshfs: { title: 'MeshFS réplique les fichiers entre les nœuds du cluster', volume: ['Volume MeshFS', 'Fichiers découpés en blocs'], nodes: [['Nœud A', 'Copies des blocs'], ['Nœud B', 'Copies des blocs'], ['Nœud C', 'Copies des blocs']], caption: 'Les nœuds répliquent les blocs. Une majorité confirme les écritures ; un nœud qui revient récupère les changements manqués.' },
    pmbs: { title: 'PMBS conserve les sauvegardes et permet leur récupération', nodes: [['Application', 'Fichiers et données', 'à sauvegarder'], ['PMBS', 'Points de sauvegarde', 'Blocs conservés'], ['Restauration', 'État choisi', 'vers une autre cible']], caption: 'On choisit un point de sauvegarde, puis on reconstruit ses fichiers à partir des blocs conservés.' },
    proxmox: { title: 'Le plugin relie Proxmox VE au stockage MeshFS', nodes: [['Proxmox VE', 'Commandes', 'de stockage'], ['Plugin', 'Adaptation', 'des opérations'], ['MeshFS', 'Volumes et fichiers', 'répliqués']], caption: 'Le plugin relie les opérations de stockage de Proxmox à MeshFS. La couche d’accès aux disques reste un élément de l’architecture.' },
    downloads: { title: 'Choisir un composant selon le besoin', nodes: [['PodMesh', 'Piloter', 'les applications'], ['MeshFS', 'Distribuer', 'les fichiers'], ['PMBS', 'Sauvegarder', 'et récupérer']], caption: 'Trois rôles complémentaires. Le guide du composant choisi donne sa procédure d’installation et son premier usage.' },
  },
  en: {
    heading: 'What each part does',
    podmesh: { title: 'PodMesh coordinates applications across hosts', nodes: [['Server A', 'Application', 'Services and data'], ['PodMesh', 'Movement', 'or replication'], ['Server B', 'Moved application', 'or replica']], caption: 'PodMesh coordinates moving an application and its data from one host to another.' },
    meshfs: { title: 'MeshFS replicates files across cluster nodes', volume: ['MeshFS volume', 'Files split into blocks'], nodes: [['Node A', 'Copies of blocks'], ['Node B', 'Copies of blocks'], ['Node C', 'Copies of blocks']], caption: 'Nodes replicate blocks. A majority confirms writes; a returning node retrieves the changes it missed.' },
    pmbs: { title: 'PMBS retains backups and supports recovery', nodes: [['Application', 'Files and data', 'to back up'], ['PMBS', 'Backup points', 'Retained blocks'], ['Restoration', 'Chosen state', 'to another destination']], caption: 'Choose a backup point, then reconstruct its files from the retained blocks.' },
    proxmox: { title: 'The plugin connects Proxmox VE to MeshFS storage', nodes: [['Proxmox VE', 'Storage', 'commands'], ['Plugin', 'Operation', 'adapter'], ['MeshFS', 'Replicated volumes', 'and files']], caption: 'The plugin connects Proxmox storage operations to MeshFS. The disk access layer remains part of the architecture.' },
    downloads: { title: 'Choose a component for the task', nodes: [['PodMesh', 'Manage', 'applications'], ['MeshFS', 'Distribute', 'files'], ['PMBS', 'Back up', 'and recover']], caption: 'Three complementary roles. Each component’s guide describes installation and a first operation.' },
  },
  es: {
    heading: 'Qué hace cada elemento',
    podmesh: { title: 'PodMesh coordina aplicaciones entre hosts', nodes: [['Servidor A', 'Aplicación', 'Servicios y datos'], ['PodMesh', 'Traslado', 'o replicación'], ['Servidor B', 'Aplicación trasladada', 'o réplica']], caption: 'PodMesh coordina el traslado de una aplicación y sus datos de un host a otro.' },
    meshfs: { title: 'MeshFS replica archivos entre los nodos del clúster', volume: ['Volumen MeshFS', 'Archivos en bloques'], nodes: [['Nodo A', 'Copias de bloques'], ['Nodo B', 'Copias de bloques'], ['Nodo C', 'Copias de bloques']], caption: 'Los nodos replican los bloques. Una mayoría confirma las escrituras; un nodo que regresa recupera los cambios pendientes.' },
    pmbs: { title: 'PMBS conserva copias y permite recuperarlas', nodes: [['Aplicación', 'Archivos y datos', 'para guardar'], ['PMBS', 'Puntos de copia', 'Bloques conservados'], ['Restauración', 'Estado elegido', 'en otro destino']], caption: 'Se elige un punto de copia y se reconstruyen sus archivos a partir de los bloques conservados.' },
    proxmox: { title: 'El plugin conecta Proxmox VE con MeshFS', nodes: [['Proxmox VE', 'Comandos', 'de almacenamiento'], ['Plugin', 'Adaptación', 'de operaciones'], ['MeshFS', 'Volúmenes y archivos', 'replicados']], caption: 'El plugin conecta las operaciones de Proxmox con MeshFS. La capa de acceso a discos sigue formando parte de la arquitectura.' },
    downloads: { title: 'Elegir un componente según la necesidad', nodes: [['PodMesh', 'Gestionar', 'aplicaciones'], ['MeshFS', 'Distribuir', 'archivos'], ['PMBS', 'Guardar copias', 'y recuperar']], caption: 'Tres funciones complementarias. La guía de cada componente describe la instalación y una primera operación.' },
  },
}

function Box({ x, y, width, height = 112, lines, accent = false }) {
  return <g>
    <rect x={x} y={y} width={width} height={height} rx="14" fill={accent ? 'var(--color-accent-soft)' : 'var(--color-surface)'} stroke={accent ? 'var(--color-accent)' : 'var(--color-line)'} strokeWidth={accent ? 2 : 1.5} />
    <text x={x + width / 2} y={y + 34} textAnchor="middle" fill="var(--color-fg)" fontSize="16" fontWeight="700">{lines[0]}</text>
    {lines.slice(1).map((line, i) => <text key={line} x={x + width / 2} y={y + 61 + i * 22} textAnchor="middle" fill="var(--color-fg-muted)" fontSize="14">{line}</text>)}
  </g>
}

export default function ComponentDiagram({ section, locale }) {
  const copy = COPY[locale] || COPY.fr
  const diagram = copy[section]
  const id = useId().replace(/:/g, '')
  const mesh = section === 'meshfs'
  function drawing(mobile) {
    const key = `${id}-${mobile ? 'mobile' : 'desktop'}`
    const width = mobile ? 320 : 720
    const height = mobile ? (mesh ? 560 : 450) : (mesh ? 290 : 215)
    const xs = mobile ? [24, 24, 24] : [20, 270, 520]
    const ys = mobile ? (mesh ? [164, 298, 432] : [12, 168, 324]) : (mesh ? [170, 170, 170] : [50, 50, 50])
    return <svg className={mobile ? 'block w-full sm:hidden' : 'hidden w-full sm:block'} viewBox={`0 0 ${width} ${height}`} role="img" aria-labelledby={`${key}-title ${key}-desc`}>
      <title id={`${key}-title`}>{diagram.title}</title>
      <desc id={`${key}-desc`}>{diagram.caption}</desc>
      <defs><marker id={`${key}-arrow`} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="var(--color-accent)" /></marker></defs>
      <g fill="none" stroke="var(--color-accent)" strokeWidth="2" markerEnd={`url(#${key}-arrow)`}>
        {mesh ? diagram.nodes.map((_, i) => <path key={i} d={mobile ? `M160 100 V134 H12 V${ys[i] + 45} H24` : `M360 100 V135 H${xs[i] + 90} V170`} />) : section !== 'downloads' && [0, 1].map(i => <path key={i} d={mobile ? `M160 ${ys[i] + 112} V${ys[i + 1] - 4}` : `M${xs[i] + 180} 106 H${xs[i + 1] - 4}`} />)}
      </g>
      {mesh && <Box x={mobile ? 24 : 250} y={12} width={mobile ? 272 : 220} height={88} lines={diagram.volume} accent />}
      {diagram.nodes.map((lines, i) => <Box key={lines[0]} x={xs[i]} y={ys[i]} width={mobile ? 272 : 180} height={mesh ? 96 : 112} lines={lines} accent={!mesh && i === 1} />)}
    </svg>
  }
  return <figure className="rounded-2xl border border-line bg-surface/70 px-3 py-5 sm:px-5 sm:py-6" data-component-diagram={section}>
    <h3 className="mb-3 px-2 text-sm font-semibold text-accent">{copy.heading}</h3>
    {drawing(false)}{drawing(true)}
    <figcaption className="mt-3 px-2 text-sm leading-6 text-fg-muted">{diagram.caption}</figcaption>
  </figure>
}
