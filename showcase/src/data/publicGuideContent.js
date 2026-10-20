// Public guide: clear component roles, following the ideal scene.
export const guideContent = {
  "fr": {
    "title": "Le plaisir d’explorer. Le souci de bien faire.",
    "intro": "Tout est parti d’une découverte avec Astra et d’une question : pourrait-on retrouver ce que nous aimons dans Proxmox, en abordant autrement ce qui nous limite ?",
    "tandem": "De fil en aiguille, cette exploration a donné naissance à PodMesh, MeshFS et PMBS. Notre fil conducteur : des outils utiles au quotidien, compréhensibles dans leur fonctionnement, et une fiabilité qui se construit et se vérifie. Pour le plaisir de s’en servir — et la tranquillité de pouvoir compter dessus.",
    "vocabulary": "Dans SHAPER OS, un « univers » rassemble une application, ses services et ses données. Nous employons ce mot pour parler de cet ensemble.",
    "summary": "Explorer les outils",
    "usage": "En clair",
    "example": "Par exemple",
    "agent": "Avec un agent IA",
    "details": "Comprendre le fonctionnement",
    "docs": "Ouvrir la documentation",
    "source": "Voir le code",
    "next": "Continuer avec",
    "previous": "Revenir à",
    "menu": "Ouvrir le sommaire",
    "close": "Fermer le menu",
    "footer": "Explorer, comprendre, construire dans la durée.",
    "meta": "Une découverte avec Astra, puis des outils pour les applications, les fichiers et les sauvegardes. Découvrez PodMesh, MeshFS et PMBS, leurs usages et leur fonctionnement.",
    "sections": [
      {
        "id": "podmesh",
        "name": "PodMesh",
        "short": "Orchestrer les conteneurs",
        "title": "Créer, déplacer et répliquer des applications.",
        "lead": "PodMesh est un orchestrateur d’applications conteneurisées avec Podman. Il rassemble les commandes pour créer une application, la démarrer, l’arrêter, la déplacer vers un autre serveur et préparer sa réplique. Il prend en compte ses services et ses données, ainsi que son état d’exécution pour les déplacements à chaud compatibles.",
        "points": [
          [
            "Piloter les applications",
            "Créer, démarrer et arrêter les conteneurs qui composent une application, puis consulter leur état depuis une console ou des outils de commande."
          ],
          [
            "Déplacer une application",
            "Transférer l’application et les données qu’elle doit retrouver sur un autre hôte. Lorsque la configuration le permet, un déplacement à chaud reprend aussi son état d’exécution."
          ],
          [
            "Préparer la reprise ailleurs",
            "Maintenir une réplique sur un autre serveur et organiser sa reprise selon les règles de continuité choisies pour l’application."
          ]
        ],
        "example": "Vous devez entretenir un serveur : PodMesh organise le déplacement de l’application vers un autre hôte avec ses services et ses données. La réplication prépare une autre situation : disposer d’une copie pour reprendre après une panne.",
        "workflow": [
          "Recenser les conteneurs et les données de l’application, puis choisir un hôte de destination.",
          "Demander le déplacement ou la réplication avec les outils PodMesh, et suivre l’opération.",
          "Vérifier que l’application répond sur le bon hôte et retrouve ses données."
        ],
        "technical": "Podman exécute les conteneurs ; PodMesh coordonne leur cycle de vie et les opérations entre hôtes. Un univers SHAPER OS regroupe une application, ses services et ses données. Déplacement, réplication et restauration ont des rôles différents. La scène idéale vise une même opération depuis la console, la CLI ou un agent IA, avec une seule instance autorisée à écrire lorsque l’application l’exige.",
        "doc": "docs/IDEAL-SCENE.md"
      },
      {
        "id": "meshfs",
        "name": "MeshFS",
        "short": "Système de fichiers distribué",
        "title": "Stocker et répliquer des fichiers sur plusieurs serveurs.",
        "lead": "MeshFS est un système de fichiers distribué : il organise les fichiers en volumes et réplique leur contenu entre plusieurs serveurs, appelés nœuds. Son rôle est de rendre les mêmes fichiers accessibles dans le cluster, de garder des copies cohérentes et de rattraper les changements manqués lorsqu’un nœud revient.",
        "points": [
          [
            "Répliquer les fichiers entre les nœuds",
            "Un fichier envoyé au cluster est découpé en blocs et répliqué. Quand un nœud manque des écritures pendant son absence, il récupère les changements à son retour."
          ],
          [
            "Éviter des versions contradictoires",
            "Les écritures suivent un ordre commun validé par une majorité de nœuds. Une machine isolée refuse une opération qu’elle ne peut pas confirmer, plutôt que de présenter une ancienne version comme actuelle."
          ],
          [
            "Réutiliser les blocs et garder des instantanés",
            "Les blocs identiques peuvent être partagés entre fichiers et versions : c’est la déduplication. Les snapshots conservent un état des fichiers sans recopier tous leurs blocs."
          ]
        ],
        "example": "Vous envoyez une image de disque au cluster pendant qu’un nœud est arrêté. Les nœuds restants conservent le fichier. Au retour du nœud absent, MeshFS lui transmet ce qu’il a manqué pour qu’il retrouve le même contenu.",
        "workflow": [
          "Choisir le volume et envoyer le fichier au cluster, par exemple une image RAW ou QCOW2.",
          "Consulter l’état des nœuds et la progression de la réplication.",
          "Lire le fichier depuis un autre nœud et vérifier son empreinte après récupération."
        ],
        "technical": "MeshFS découpe les fichiers en blocs de 4 Mio, identifiés par leur empreinte SHA-256 : c’est le stockage adressé par le contenu, ou CAS. Un arbre de Merkle décrit les fichiers et leurs changements. Le mode distribué utilise un journal répliqué et une majorité pour ordonner les écritures ; le rattrapage passe par ce journal ou un snapshot. Les transferts de grandes images passent par la CLI ou l’API en flux. L’accès en direct aux disques d’une VM en exécution nécessite aussi une couche de montage ou de stockage bloc.",
        "doc": "crates/meshfs",
        "docLabel": "Voir le code source de MeshFS"
      },
      {
        "id": "pmbs",
        "name": "PMBS",
        "short": "Serveur de sauvegarde",
        "title": "Sauvegarder les données et retrouver un état antérieur.",
        "lead": "PMBS, PodMesh Backup Server, est le serveur de sauvegarde du projet. Il conserve des points de sauvegarde, les fichiers qu’ils contiennent et les blocs nécessaires pour les reconstruire. Il permet de contrôler leur intégrité et de préparer une restauration vers un emplacement choisi.",
        "points": [
          [
            "Conserver des points de sauvegarde",
            "Retrouver les fichiers associés à une sauvegarde identifiée, pour choisir l’état que l’on veut récupérer."
          ],
          [
            "Limiter les copies inutiles",
            "Les sauvegardes partagent leurs blocs identiques. Les transferts entre sites peuvent porter sur les blocs manquants, plutôt que sur toutes les données."
          ],
          [
            "Restaurer et vérifier le résultat",
            "Récupérer les fichiers d’un point de sauvegarde, puis contrôler les données et le fonctionnement de l’application restaurée."
          ]
        ],
        "example": "Une mise à jour pose problème. Vous choisissez une sauvegarde antérieure, restaurez ses données dans une cible distincte et vérifiez l’application avant de décider de la suite.",
        "workflow": [
          "Lister les points de sauvegarde et choisir celui à restaurer.",
          "Vérifier ses blocs et préparer une cible de restauration.",
          "Restaurer les fichiers, puis vérifier les données et le service qui les utilise."
        ],
        "technical": "PMBS conserve des manifestes de sauvegarde : ils décrivent quels fichiers et quels blocs appartiennent à chaque point de reprise. Les blocs sont adressés par leur contenu pour la déduplication et la vérification d’intégrité. La scène idéale prévoit une conservation extérieure aux hôtes applicatifs et une copie sur un autre site. MeshFS sert au stockage distribué des fichiers ; PMBS sert à retrouver un état sauvegardé après une erreur, une perte ou un incident.",
        "doc": "crates/pmbs",
        "docLabel": "Voir le code source de PMBS"
      },
      {
        "id": "proxmox",
        "name": "Proxmox VE",
        "short": "Connecter MeshFS à Proxmox",
        "title": "Gérer le stockage MeshFS depuis Proxmox VE.",
        "lead": "Le plugin Proxmox VE est l’adaptateur entre MeshFS et les outils de stockage de Proxmox. Il décrit un stockage, expose son état et relie les opérations sur les volumes et les images de disques aux interfaces de Proxmox, afin de les retrouver dans cet environnement.",
        "points": [
          [
            "Déclarer le stockage",
            "Renseigner le serveur, le volume, le chemin et les types de contenus concernés : images de disques, ISO ou sauvegardes."
          ],
          [
            "Gérer les images et les instantanés",
            "Relier les opérations de création, de liste et de snapshot des images aux commandes de stockage prises en charge par le plugin."
          ],
          [
            "Consulter l’état et la capacité",
            "Retrouver l’espace disponible et l’état du stockage depuis les interfaces de Proxmox."
          ]
        ],
        "example": "Vous administrez vos VM avec Proxmox. Le plugin permet de déclarer un stockage MeshFS et de retrouver ses opérations de stockage parmi vos outils habituels.",
        "workflow": [
          "Lire la configuration du stockage Proxmox et identifier le volume MeshFS associé.",
          "Préparer la déclaration du stockage et les types de contenus nécessaires.",
          "Exécuter les opérations prises en charge et vérifier les images et les données correspondantes."
        ],
        "technical": "Le plugin utilise PVE::Storage::Plugin, le mécanisme d’extension du stockage de Proxmox VE. Il adapte les commandes de Proxmox aux opérations de MeshFS et aux outils d’images comme qemu-img. Déclarer un stockage partagé ne réplique pas ses données : la réplication relève de MeshFS. L’accès réel d’une VM en exécution dépend aussi de la couche de montage ou de stockage bloc utilisée.",
        "doc": "packaging/proxmox/README-PROXMOX-PLUGIN.md"
      },
      {
        "id": "downloads",
        "name": "Premiers pas",
        "short": "Installation et premier usage",
        "title": "Choisir les composants, les installer et les utiliser.",
        "lead": "PodMesh, MeshFS et PMBS répondent à trois besoins : piloter les applications, répartir leurs fichiers et conserver leurs sauvegardes. Les guides d’installation donnent les composants et les prérequis nécessaires selon l’usage choisi. Vous pouvez commencer par un outil sans déployer tout l’ensemble.",
        "points": [
          [
            "Choisir l’outil adapté",
            "PodMesh pour gérer les applications Podman ; MeshFS pour le système de fichiers distribué ; PMBS pour les sauvegardes et leur récupération."
          ],
          [
            "Préparer l’installation",
            "Repérer la version, les prérequis et la procédure du composant, puis identifier les machines et les espaces de données qui l’accueilleront."
          ],
          [
            "Réaliser une première opération",
            "Démarrer une application, transférer un fichier entre nœuds ou restaurer une sauvegarde : choisir une action concrète et vérifier son résultat."
          ]
        ],
        "example": "Pour découvrir MeshFS, vous préparez un volume de test sur plusieurs nœuds, envoyez un fichier depuis l’un d’eux et le relisez depuis un autre. Pour PMBS, vous partez d’un point de sauvegarde et récupérez un fichier.",
        "workflow": [
          "Choisir le composant en fonction du résultat attendu.",
          "Suivre son guide d’installation et vérifier que le service répond.",
          "Effectuer une première opération avec des données de test, puis en contrôler le résultat."
        ],
        "technical": "Le projet sépare l’orchestration des applications, le stockage distribué et les sauvegardes. Les commandes et les API sont les points d’entrée des utilisateurs comme des agents IA. Les procédures d’installation précisent les versions et les composants nécessaires pour chaque rôle.",
        "doc": "docs/README.md"
      }
    ],
    "direction": "Cette vitrine présente la direction du projet : ce que nous construisons et les usages qui nous guident.",
    "directionLink": "Lire la scène idéale",
    "overview": "PodMesh pilote les applications. MeshFS distribue leurs fichiers entre serveurs. PMBS conserve leurs sauvegardes."
  },
  "en": {
    "title": "The joy of exploring. The care to do things well.",
    "intro": "It began with a discovery alongside Astra and a question: could we keep what we enjoy about Proxmox while taking a different approach to what limits us?",
    "tandem": "Step by step, that exploration gave rise to PodMesh, MeshFS and PMBS. Our guiding thread: tools that are useful every day, understandable in how they work, with reliability that is built and checked. For the pleasure of using them — and the peace of mind of being able to count on them.",
    "vocabulary": "In SHAPER OS, a “universe” brings together an application, its services and its data. We use this word to describe that whole.",
    "summary": "Explore the tools",
    "usage": "In plain language",
    "example": "For example",
    "agent": "With an AI agent",
    "details": "How it works",
    "docs": "Open the documentation",
    "source": "Read the code",
    "next": "Continue with",
    "previous": "Back to",
    "menu": "Open contents",
    "close": "Close menu",
    "footer": "Explore, understand, build for the long run.",
    "meta": "A discovery alongside Astra, then tools for applications, files and backups. Explore PodMesh, MeshFS and PMBS, their uses and how they work.",
    "sections": [
      {
        "id": "podmesh",
        "name": "PodMesh",
        "short": "Container orchestration",
        "title": "Create, move and replicate applications.",
        "lead": "PodMesh is an orchestrator for applications running in Podman containers. It brings together commands to create, start and stop an application, move it to another server and prepare a replica. It handles its services and data, and its running state for compatible live moves.",
        "points": [
          [
            "Manage applications",
            "Create, start and stop the containers that make up an application, and inspect their state from a console or command tools."
          ],
          [
            "Move an application",
            "Transfer the application and its required data to another host. Compatible live moves also resume its running state."
          ],
          [
            "Prepare recovery elsewhere",
            "Maintain a replica on another server and organise recovery according to the application’s continuity rules."
          ]
        ],
        "example": "A server needs maintenance: PodMesh organises the application’s move with its services and data. Replication prepares for a different situation: having a copy available for recovery after a failure.",
        "workflow": [
          "Identify the application’s containers and data, then choose a destination host.",
          "Request a move or replication through PodMesh and follow the operation.",
          "Check that the application responds on the right host and finds its data."
        ],
        "technical": "Podman runs containers; PodMesh coordinates their lifecycle and operations between hosts. A SHAPER OS universe groups an application, its services and data. Movement, replication and restoration serve different purposes. The ideal scene provides the same operation through a console, CLI or AI agent, with a single authorised writer where required.",
        "doc": "docs/IDEAL-SCENE.md"
      },
      {
        "id": "meshfs",
        "name": "MeshFS",
        "short": "Distributed filesystem",
        "title": "Store and replicate files across servers.",
        "lead": "MeshFS is a distributed filesystem: it organises files into volumes and replicates their content across servers called nodes. Its role is to make the same files available across the cluster, keep copies consistent and catch up on missed changes when a node returns.",
        "points": [
          [
            "Replicate files between nodes",
            "Files sent to the cluster are split into blocks and replicated. A node that misses writes during an absence retrieves those changes when it returns."
          ],
          [
            "Avoid conflicting versions",
            "Writes follow a shared order validated by a majority of nodes. An isolated machine refuses an operation it cannot confirm instead of presenting an old version as current."
          ],
          [
            "Reuse blocks and retain snapshots",
            "Identical blocks can be shared across files and versions: this is deduplication. Snapshots retain file states without copying every block."
          ]
        ],
        "example": "You send a disk image to the cluster while one node is stopped. The remaining nodes retain the file. When the absent node returns, MeshFS sends what it missed so it can recover the same content.",
        "workflow": [
          "Choose a volume and send a file to the cluster, such as a RAW or QCOW2 image.",
          "Inspect node state and replication progress.",
          "Read the file through another node and check its fingerprint after recovery."
        ],
        "technical": "MeshFS splits files into 4 MiB blocks identified by SHA-256 fingerprints: content-addressed storage, or CAS. A Merkle tree describes files and changes. Distributed mode uses a replicated log and a majority to order writes; returning nodes catch up through that log or a snapshot. Large images transfer through the streaming CLI or API. Direct disk access for a running VM also requires a filesystem mount or block-storage layer.",
        "doc": "crates/meshfs",
        "docLabel": "Read the MeshFS source code"
      },
      {
        "id": "pmbs",
        "name": "PMBS",
        "short": "Backup server",
        "title": "Back up data and recover an earlier state.",
        "lead": "PMBS, PodMesh Backup Server, is the project’s backup server. It retains backup points, their files and the blocks needed to reconstruct them. It checks their integrity and prepares restoration to a chosen destination.",
        "points": [
          [
            "Keep backup points",
            "Find the files associated with an identified backup and choose the state to recover."
          ],
          [
            "Avoid unnecessary copies",
            "Backups share identical blocks. Transfers between sites can send missing blocks instead of all the data."
          ],
          [
            "Restore and check the result",
            "Recover a backup’s files, then check the data and the restored application."
          ]
        ],
        "example": "An update causes a problem. Choose an earlier backup, restore its data to a separate destination and check the application before deciding what to do next.",
        "workflow": [
          "List backup points and choose one to restore.",
          "Verify its blocks and prepare a restoration destination.",
          "Restore files, then check the data and the service that uses it."
        ],
        "technical": "PMBS stores backup manifests describing the files and blocks belonging to each recovery point. Content-addressed blocks enable deduplication and integrity checks. The ideal scene places backups outside application hosts, with a copy at another site. MeshFS provides distributed file storage; PMBS recovers a saved state after an error, loss or incident.",
        "doc": "crates/pmbs",
        "docLabel": "Read the PMBS source code"
      },
      {
        "id": "proxmox",
        "name": "Proxmox VE",
        "short": "Connect MeshFS to Proxmox",
        "title": "Manage MeshFS storage from Proxmox VE.",
        "lead": "The Proxmox VE plugin is the adapter between MeshFS and Proxmox storage tools. It describes storage, exposes its state and connects volume and disk-image operations to Proxmox interfaces.",
        "points": [
          [
            "Declare storage",
            "Set the server, volume, path and content types: disk images, ISO files or backups."
          ],
          [
            "Manage images and snapshots",
            "Connect image creation, listing and snapshot operations to storage commands supported by the plugin."
          ],
          [
            "Inspect state and capacity",
            "Find available space and storage state through Proxmox interfaces."
          ]
        ],
        "example": "You administer VMs with Proxmox. The plugin lets you declare MeshFS storage and find its storage operations among your familiar tools.",
        "workflow": [
          "Read the Proxmox storage configuration and identify its MeshFS volume.",
          "Prepare the storage declaration and required content types.",
          "Run supported operations and check the corresponding images and data."
        ],
        "technical": "The plugin uses PVE::Storage::Plugin, Proxmox VE’s storage extension mechanism. It adapts Proxmox commands to MeshFS operations and image tools such as qemu-img. Declaring shared storage does not replicate data: replication belongs to MeshFS. Actual access by a running VM also depends on the filesystem mount or block-storage layer.",
        "doc": "packaging/proxmox/README-PROXMOX-PLUGIN.md"
      },
      {
        "id": "downloads",
        "name": "Getting started",
        "short": "Installation and first use",
        "title": "Choose components, install them and use them.",
        "lead": "PodMesh, MeshFS and PMBS meet three needs: managing applications, distributing files and retaining backups. Installation guides describe the components and prerequisites for each use. You can begin with one tool without deploying everything.",
        "points": [
          [
            "Choose the appropriate tool",
            "PodMesh manages Podman applications; MeshFS provides the distributed filesystem; PMBS handles backups and recovery."
          ],
          [
            "Prepare installation",
            "Find the component version, prerequisites and procedure, then identify its hosts and data storage."
          ],
          [
            "Perform a first operation",
            "Start an application, transfer a file between nodes or restore a backup: choose a concrete action and check the result."
          ]
        ],
        "example": "To discover MeshFS, prepare a test volume across nodes, send a file through one and read it through another. With PMBS, start from a backup point and recover a file.",
        "workflow": [
          "Choose a component for the desired result.",
          "Follow its installation guide and check that the service responds.",
          "Perform a first operation with test data and verify the result."
        ],
        "technical": "The project separates application orchestration, distributed storage and backups. Commands and APIs are entry points for users and AI agents alike. Installation procedures specify the versions and components needed for each role.",
        "doc": "docs/README.md"
      }
    ],
    "direction": "This guide presents the direction of the project: what we are building and the uses that guide us.",
    "directionLink": "Read the ideal scene",
    "overview": "PodMesh manages applications. MeshFS distributes their files across servers. PMBS keeps their backups."
  },
  "es": {
    "title": "El placer de explorar. El cuidado de hacer las cosas bien.",
    "intro": "Todo empezó con un descubrimiento junto a Astra y una pregunta: ¿podríamos conservar lo que nos gusta de Proxmox y abordar de otra manera lo que nos limita?",
    "tandem": "Paso a paso, esa exploración dio lugar a PodMesh, MeshFS y PMBS. Nuestro hilo conductor: herramientas útiles a diario, comprensibles en su funcionamiento y una fiabilidad que se construye y se comprueba. Por el placer de usarlas y la tranquilidad de poder contar con ellas.",
    "vocabulary": "En SHAPER OS, un «universo» reúne una aplicación, sus servicios y sus datos. Usamos esta palabra para referirnos al conjunto.",
    "summary": "Explorar las herramientas",
    "usage": "En palabras sencillas",
    "example": "Por ejemplo",
    "agent": "Con un agente de IA",
    "details": "Cómo funciona",
    "docs": "Abrir la documentación",
    "source": "Ver el código",
    "next": "Continuar con",
    "previous": "Volver a",
    "menu": "Abrir el índice",
    "close": "Cerrar el menú",
    "footer": "Explorar, comprender, construir para durar.",
    "meta": "Un descubrimiento junto a Astra y después herramientas para aplicaciones, archivos y copias de seguridad. Descubre PodMesh, MeshFS y PMBS, sus usos y cómo funcionan.",
    "sections": [
      {
        "id": "podmesh",
        "name": "PodMesh",
        "short": "Orquestar contenedores",
        "title": "Crear, trasladar y replicar aplicaciones.",
        "lead": "PodMesh es un orquestador de aplicaciones en contenedores Podman. Reúne las operaciones para crear, iniciar y detener una aplicación, trasladarla a otro servidor y preparar una réplica. Gestiona sus servicios y datos, y su estado de ejecución en traslados en caliente compatibles.",
        "points": [
          [
            "Gestionar aplicaciones",
            "Crear, iniciar y detener los contenedores de una aplicación y consultar su estado desde una consola o herramientas de comandos."
          ],
          [
            "Trasladar una aplicación",
            "Transferir la aplicación y sus datos a otro host. Los traslados en caliente compatibles también reanudan su estado de ejecución."
          ],
          [
            "Preparar la recuperación",
            "Mantener una réplica en otro servidor y organizar su recuperación según las reglas de continuidad de la aplicación."
          ]
        ],
        "example": "Un servidor necesita mantenimiento: PodMesh organiza el traslado de la aplicación con sus servicios y datos. La replicación prepara otra situación: disponer de una copia para recuperarse tras una avería.",
        "workflow": [
          "Identificar los contenedores y datos de la aplicación y elegir un host de destino.",
          "Solicitar el traslado o la replicación con PodMesh y seguir la operación.",
          "Comprobar que la aplicación responde en el host correcto y encuentra sus datos."
        ],
        "technical": "Podman ejecuta los contenedores; PodMesh coordina su ciclo de vida y las operaciones entre hosts. Un universo SHAPER OS agrupa una aplicación, sus servicios y datos. Traslado, replicación y restauración cumplen funciones distintas. La escena ideal ofrece la misma operación desde consola, CLI o un agente de IA, con una única instancia autorizada a escribir cuando sea necesario.",
        "doc": "docs/IDEAL-SCENE.md"
      },
      {
        "id": "meshfs",
        "name": "MeshFS",
        "short": "Sistema de archivos distribuido",
        "title": "Almacenar y replicar archivos entre servidores.",
        "lead": "MeshFS es un sistema de archivos distribuido: organiza archivos en volúmenes y replica su contenido entre servidores llamados nodos. Su función es ofrecer los mismos archivos en el clúster, mantener copias coherentes y recuperar los cambios pendientes cuando un nodo regresa.",
        "points": [
          [
            "Replicar archivos entre nodos",
            "Los archivos enviados al clúster se dividen en bloques y se replican. Un nodo que pierde escrituras durante su ausencia recupera los cambios al regresar."
          ],
          [
            "Evitar versiones contradictorias",
            "Las escrituras siguen un orden común validado por una mayoría de nodos. Una máquina aislada rechaza una operación que no puede confirmar, en lugar de presentar una versión antigua como actual."
          ],
          [
            "Reutilizar bloques y conservar instantáneas",
            "Los bloques idénticos pueden compartirse entre archivos y versiones: es la deduplicación. Las instantáneas conservan estados de archivos sin copiar todos los bloques."
          ]
        ],
        "example": "Envías una imagen de disco al clúster mientras un nodo está detenido. Los demás conservan el archivo. Cuando vuelve el nodo ausente, MeshFS le envía lo que le falta para recuperar el mismo contenido.",
        "workflow": [
          "Elegir un volumen y enviar un archivo al clúster, por ejemplo una imagen RAW o QCOW2.",
          "Consultar el estado de los nodos y el progreso de replicación.",
          "Leer el archivo desde otro nodo y comprobar su huella después de recuperarlo."
        ],
        "technical": "MeshFS divide los archivos en bloques de 4 MiB identificados por huellas SHA-256: almacenamiento direccionado por contenido, o CAS. Un árbol de Merkle describe los archivos y sus cambios. El modo distribuido usa un registro replicado y una mayoría para ordenar escrituras; el nodo recupera cambios mediante ese registro o una instantánea. Las imágenes grandes se transfieren mediante la CLI o API en flujo. El acceso directo al disco de una VM en ejecución también necesita una capa de montaje o almacenamiento de bloques.",
        "doc": "crates/meshfs",
        "docLabel": "Ver el código fuente de MeshFS"
      },
      {
        "id": "pmbs",
        "name": "PMBS",
        "short": "Servidor de copias de seguridad",
        "title": "Guardar datos y recuperar un estado anterior.",
        "lead": "PMBS, PodMesh Backup Server, es el servidor de copias de seguridad del proyecto. Conserva puntos de copia, sus archivos y los bloques necesarios para reconstruirlos. Permite comprobar su integridad y preparar la restauración en un destino elegido.",
        "points": [
          [
            "Conservar puntos de copia",
            "Encontrar los archivos asociados a una copia identificada y elegir el estado que recuperar."
          ],
          [
            "Evitar copias innecesarias",
            "Las copias comparten bloques idénticos. Los traslados entre sitios pueden enviar solo los bloques que faltan."
          ],
          [
            "Restaurar y comprobar",
            "Recuperar los archivos de una copia y comprobar los datos y la aplicación restaurada."
          ]
        ],
        "example": "Una actualización causa un problema. Eliges una copia anterior, restauras sus datos en otro destino y compruebas la aplicación antes de decidir cómo continuar.",
        "workflow": [
          "Listar los puntos de copia y elegir uno para restaurar.",
          "Verificar los bloques y preparar el destino de restauración.",
          "Restaurar los archivos y comprobar los datos y el servicio que los utiliza."
        ],
        "technical": "PMBS guarda manifiestos que describen los archivos y bloques de cada punto de recuperación. Los bloques direccionados por contenido permiten deduplicación y comprobación de integridad. La escena ideal sitúa las copias fuera de los hosts de aplicaciones, con otra copia en un segundo sitio. MeshFS ofrece almacenamiento distribuido; PMBS permite recuperar un estado guardado tras un error, una pérdida o un incidente.",
        "doc": "crates/pmbs",
        "docLabel": "Ver el código fuente de PMBS"
      },
      {
        "id": "proxmox",
        "name": "Proxmox VE",
        "short": "Conectar MeshFS con Proxmox",
        "title": "Gestionar MeshFS desde Proxmox VE.",
        "lead": "El plugin Proxmox VE es el adaptador entre MeshFS y las herramientas de almacenamiento de Proxmox. Describe el almacenamiento, muestra su estado y conecta las operaciones de volúmenes e imágenes de disco con las interfaces de Proxmox.",
        "points": [
          [
            "Declarar el almacenamiento",
            "Configurar servidor, volumen, ruta y tipos de contenido: imágenes de disco, ISO o copias."
          ],
          [
            "Gestionar imágenes e instantáneas",
            "Conectar creación, listado e instantáneas de imágenes con las operaciones compatibles con el plugin."
          ],
          [
            "Consultar estado y capacidad",
            "Encontrar el espacio disponible y el estado desde las interfaces de Proxmox."
          ]
        ],
        "example": "Administras tus VM con Proxmox. El plugin permite declarar almacenamiento MeshFS y encontrar sus operaciones entre las herramientas habituales.",
        "workflow": [
          "Leer la configuración de Proxmox e identificar su volumen MeshFS.",
          "Preparar la declaración de almacenamiento y los tipos de contenido necesarios.",
          "Ejecutar las operaciones compatibles y comprobar sus imágenes y datos."
        ],
        "technical": "El plugin usa PVE::Storage::Plugin, el mecanismo de extensión de almacenamiento de Proxmox VE. Adapta comandos de Proxmox a MeshFS y herramientas como qemu-img. Declarar almacenamiento compartido no replica los datos: esa tarea corresponde a MeshFS. El acceso de una VM en ejecución también depende de la capa de montaje o almacenamiento de bloques.",
        "doc": "packaging/proxmox/README-PROXMOX-PLUGIN.md"
      },
      {
        "id": "downloads",
        "name": "Primeros pasos",
        "short": "Instalación y primer uso",
        "title": "Elegir componentes, instalarlos y utilizarlos.",
        "lead": "PodMesh, MeshFS y PMBS cubren tres necesidades: gestionar aplicaciones, distribuir archivos y conservar copias. Las guías de instalación describen los componentes y requisitos de cada uso. Puedes empezar con una herramienta sin desplegar todo el conjunto.",
        "points": [
          [
            "Elegir la herramienta",
            "PodMesh para aplicaciones Podman; MeshFS para el sistema de archivos distribuido; PMBS para copias y recuperación."
          ],
          [
            "Preparar la instalación",
            "Consultar versión, requisitos y procedimiento e identificar las máquinas y espacios de datos."
          ],
          [
            "Realizar una primera operación",
            "Iniciar una aplicación, transferir un archivo entre nodos o restaurar una copia y comprobar el resultado."
          ]
        ],
        "example": "Para descubrir MeshFS, preparas un volumen de prueba entre nodos, envías un archivo desde uno y lo lees desde otro. Con PMBS, eliges un punto de copia y recuperas un archivo.",
        "workflow": [
          "Elegir el componente según el resultado esperado.",
          "Seguir su guía de instalación y comprobar que el servicio responde.",
          "Realizar una operación con datos de prueba y verificar el resultado."
        ],
        "technical": "El proyecto separa orquestación, almacenamiento distribuido y copias de seguridad. Los comandos y las API son puntos de entrada para personas y agentes de IA. Las guías indican versiones y componentes para cada función.",
        "doc": "docs/README.md"
      }
    ],
    "direction": "Esta presentación muestra el rumbo del proyecto: lo que construimos y los usos que nos guían.",
    "directionLink": "Leer la escena ideal",
    "overview": "PodMesh gestiona las aplicaciones. MeshFS distribuye sus archivos entre servidores. PMBS conserva sus copias de seguridad."
  }
}
