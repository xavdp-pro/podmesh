// Public guide follows IDEAL-SCENE.md: direction and uses, not a live qualification report.
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
        "short": "Déplacer et répliquer les applications",
        "title": "Une application, au-delà de son premier serveur.",
        "lead": "Une application vit sur une machine, mais elle ne se résume pas à son emplacement. Elle a des services, des fichiers et un état d’exécution : ce qu’elle est en train de faire. Avec PodMesh et Podman, nous explorons comment gérer cet ensemble, le déplacer à chaud et en préparer une réplique, avec le souci de retrouver ce qui le fait fonctionner.",
        "points": [
          [
            "Voir ce que l’on prend en charge",
            "Retrouver les conteneurs, les services et les données qui font vivre l’application. Dans SHAPER OS, cet ensemble s’appelle un univers."
          ],
          [
            "Explorer le déplacement à chaud",
            "Capturer un état d’exécution et le reprendre sur une autre machine : on s’intéresse à ce que l’application faisait, en plus de ses fichiers. Le déroulement dépend de la configuration."
          ],
          [
            "Préparer une copie pour la reprise",
            "La réplication maintient une copie sur un autre serveur. Déplacer une application et la répliquer répondent à deux besoins différents, que nous travaillons dans le même projet."
          ]
        ],
        "example": "Un serveur a besoin d’entretien. Vous voulez pouvoir préparer le passage de votre application sur une autre machine, retrouver ses données et vérifier qu’elle fonctionne avant de poursuivre. C’est un besoin quotidien, et une bonne raison de travailler le déplacement et la reprise avec soin.",
        "workflow": [
          "Observer l’état réel des applications et les ressources disponibles.",
          "Préparer l’opération avec les interfaces de la version installée et les autorisations reçues.",
          "Contrôler l’application et ses données après l’opération, puis expliquer ce qui a réussi ou reste à faire."
        ],
        "technical": "La scène idéale donne au tandem humain-agent un même contrat pour créer, démarrer, déplacer, protéger et restaurer un univers, depuis la console comme depuis les outils d’un agent. Les hôtes portent le fonctionnement : un poste de travail sert à observer et à commander. Chaque univers déclare ses règles de continuité et les données à préserver. Les décisions exclusives doivent éviter deux instances qui écrivent des histoires différentes ; une opération interrompue doit pouvoir reprendre sans répéter ses effets. Les possibilités de reprise suivent l’infrastructure disponible.",
        "doc": "docs/IDEAL-SCENE.md"
      },
      {
        "id": "meshfs",
        "name": "MeshFS",
        "short": "Garder la trace des fichiers",
        "title": "Garder la trace, sans tout recopier.",
        "lead": "Les fichiers changent, et l’on aimerait pouvoir retrouver leur histoire sans conserver une copie complète à chaque étape. MeshFS part de cette idée : organiser le contenu en blocs reconnus par leur empreinte numérique, puis réutiliser ceux qui sont déjà présents. Une piste passionnante à explorer, à partir de fichiers tout simples.",
        "points": [
          [
            "Réutiliser ce qui existe déjà",
            "Deux versions contiennent les mêmes blocs ? MeshFS peut les conserver une seule fois et les réutiliser. C’est la déduplication. La place gagnée dépend du contenu commun."
          ],
          [
            "Conserver des points de repère",
            "Un instantané, ou snapshot, décrit les fichiers à un moment donné. Il permet de comparer des versions et de retrouver un état avec une référence pour s’y repérer."
          ],
          [
            "Comprendre en essayant",
            "Un dossier de test permet de découvrir le fonctionnement. MeshFS peut être utilisé sans PodMesh ; le travail entre plusieurs serveurs ajoute le partage et la synchronisation."
          ]
        ],
        "example": "Vous faites évoluer un dossier au fil des jours. Certaines parties changent, d’autres restent identiques. MeshFS peut réutiliser les blocs communs entre les versions : vous pouvez alors examiner ce qui a changé et retrouver vos points de repère.",
        "workflow": [
          "Identifier les fichiers, le stockage utilisé et la place disponible.",
          "Créer ou examiner un instantané en conservant sa référence.",
          "Comparer les empreintes et vérifier les fichiers retrouvés. Pour une copie sur un autre serveur, contrôler aussi sa présence et sa lecture."
        ],
        "technical": "L’approche de MeshFS repose sur le stockage adressé par le contenu, ou CAS : des blocs identifiés par leur empreinte SHA-256 peuvent être réutilisés. Une structure de Merkle relie les fichiers à leurs blocs et aide à repérer les changements. Ce travail sur les fichiers complète celui de PodMesh sur les applications et celui de PMBS sur les sauvegardes. Le partage du stockage, les copies pour la continuité et les sauvegardes répondent à des besoins différents : nous voulons que chacun reste compréhensible.",
        "doc": "crates/meshfs",
        "docLabel": "Voir le code source de MeshFS"
      },
      {
        "id": "pmbs",
        "name": "PMBS",
        "short": "Sauvegarder pour pouvoir repartir",
        "title": "Une sauvegarde dont on sait se servir.",
        "lead": "Ce que l’on attend d’une sauvegarde, c’est de pouvoir retrouver ses données au moment où l’on en a besoin. Avec PMBS — PodMesh Backup Server — nous travaillons sur tout ce chemin : conserver les fichiers, contrôler leur intégrité et préparer leur restauration. Le but est simple : savoir comment repartir.",
        "points": [
          [
            "Retrouver une sauvegarde",
            "Identifier le contenu conservé et le point de sauvegarde auquel il correspond."
          ],
          [
            "Réutiliser les blocs identiques",
            "Les sauvegardes peuvent partager les blocs qu’elles ont en commun. La place nécessaire dépend des données et de leurs changements."
          ],
          [
            "Essayer la récupération",
            "Contrôler les blocs, restaurer dans une cible de test et vérifier que les fichiers puis l’application sont utilisables. Chaque étape apporte sa propre preuve, et c’est leur réunion qui donne confiance."
          ]
        ],
        "example": "Avant une mise à jour, vous gardez une sauvegarde et vous essayez sa restauration dans un espace de test. Retrouver les fichiers, puis faire fonctionner l’application, donne des repères concrets pour le jour où vous en aurez besoin.",
        "workflow": [
          "Repérer le contenu sauvegardé, sa référence et les éléments nécessaires à sa récupération.",
          "Contrôler la présence des blocs et leur intégrité.",
          "Restaurer dans une cible autorisée, puis vérifier les données et le fonctionnement de l’application."
        ],
        "technical": "Dans la scène idéale, un serveur de sauvegarde extérieur au groupe de machines conserve des points de reprise versionnés, protégés contre la modification, avec une copie sur un autre site. Le chemin de retour couvre les fichiers, les bases de données et les éléments nécessaires au fonctionnement de l’application. Contrôler des empreintes sert à vérifier l’intégrité ; le cap est aussi de restaurer pour de vrai et de vérifier que l’application fonctionne. C’est cette chaîne complète qui doit permettre de compter sur la sauvegarde.",
        "doc": "crates/pmbs",
        "docLabel": "Voir le code source de PMBS"
      },
      {
        "id": "proxmox",
        "name": "Proxmox VE",
        "short": "Relier les outils de stockage",
        "title": "Explorer MeshFS dans un environnement familier.",
        "lead": "Proxmox fait partie des outils que nous aimons utiliser. Le plugin du projet relie ses outils de stockage à MeshFS : une façon de prolonger l’exploration depuis une interface familière, puis de voir comment cette approche répond à vos usages.",
        "points": [
          [
            "Retrouver vos repères",
            "Passer par les interfaces de stockage de Proxmox pour les opérations prises en charge par le plugin."
          ],
          [
            "Préparer un espace de stockage",
            "Définir son emplacement, les serveurs concernés et les types de contenus autorisés."
          ],
          [
            "Essayer sur un volume de test",
            "Vérifier la création, l’écriture, la lecture et la récupération avant d’y placer une machine importante."
          ]
        ],
        "example": "Vous connaissez déjà Proxmox et vous voulez découvrir MeshFS. Vous commencez par un volume de test, vous y écrivez des données, vous les relisez et vous essayez de les retrouver après restauration. L’exploration prend ainsi appui sur des gestes connus.",
        "workflow": [
          "Vérifier les versions de Proxmox, de MeshFS et du plugin.",
          "Préparer la configuration de stockage dans le périmètre autorisé.",
          "Tester les opérations nécessaires avec de vraies données et conserver le résultat des vérifications."
        ],
        "technical": "Le plugin fait le lien entre l’interface de stockage de Proxmox VE et MeshFS, grâce au mécanisme PVE::Storage::Plugin. L’intention est de retrouver les opérations de stockage dans des outils familiers, avec des responsabilités lisibles. Le plugin est développé dans le projet PodMesh. Le stockage et les règles de continuité de l’application restent deux sujets distincts.",
        "doc": "packaging/proxmox/README-PROXMOX-PLUGIN.md"
      },
      {
        "id": "downloads",
        "name": "Premiers pas",
        "short": "Choisir quoi installer",
        "title": "Commencer petit, apprendre en faisant.",
        "lead": "Un dossier, un conteneur ou une sauvegarde de test : il suffit d’un usage précis pour commencer à comprendre. Nous vous invitons à explorer un composant, observer ce qu’il fait et prendre vos repères avant d’aller plus loin.",
        "points": [
          [
            "Choisir un point de départ",
            "Comprendre les conteneurs avec PodMesh, explorer les fichiers avec MeshFS ou préparer une sauvegarde avec PMBS."
          ],
          [
            "Lire la procédure correspondante",
            "Repérer les prérequis, la version et les composants nécessaires dans la documentation."
          ],
          [
            "Faire un premier essai",
            "Utiliser des données de test, noter ce qui a été installé et vérifier le résultat avant d’élargir l’usage."
          ]
        ],
        "example": "Pour découvrir MeshFS, vous commencez avec un dossier dont vous connaissez les fichiers. Vous créez un instantané, modifiez quelques éléments et regardez ce qui a changé. Ce premier essai aide à comprendre ce que l’outil peut vous apporter.",
        "workflow": [
          "Clarifier le résultat attendu et vérifier les prérequis de la machine.",
          "Préparer une installation à partir d’une source identifiée, avec ses versions et ses empreintes.",
          "Rendre compte de ce qui est installé, de ce qui fonctionne et des étapes encore nécessaires."
        ],
        "technical": "Le projet sépare la gestion des applications, les outils de commande, le stockage MeshFS et le serveur de sauvegarde PMBS. Les guides d’installation accompagnent le choix des composants, des versions et des prérequis. Notre direction reste la même : pouvoir comprendre ce qui a été installé, observer ses opérations et disposer d’un chemin de récupération.",
        "doc": "docs/README.md"
      }
    ],
    "direction": "Cette vitrine présente la direction du projet : ce que nous construisons et les usages qui nous guident.",
    "directionLink": "Lire la scène idéale"
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
        "short": "Move and replicate applications",
        "title": "An application, beyond its first server.",
        "lead": "An application lives on a machine, but its location is only part of the story. It has services, files and a running state: what it is doing right now. With PodMesh and Podman, we explore how to manage that whole, move it live and prepare a replica, taking care to recover what keeps it working.",
        "points": [
          [
            "See what you are looking after",
            "Identify the containers, services and data that keep the application alive. In SHAPER OS, this whole is called a universe."
          ],
          [
            "Explore live movement",
            "Capture a running state and resume it on another machine: the application’s activity matters alongside its files. The process depends on the configuration."
          ],
          [
            "Prepare a copy for recovery",
            "Replication maintains a copy on another server. Moving and replicating an application meet different needs that we work on within the same project."
          ]
        ],
        "example": "A server needs maintenance. You want to prepare your application’s move to another machine, find its data and check that it works before continuing. It is an everyday need, and a good reason to work carefully on movement and recovery.",
        "workflow": [
          "Observe the actual application state and available resources.",
          "Prepare the operation using the installed version’s interfaces and the permissions received.",
          "Check the application and data afterwards, then explain what worked and what remains."
        ],
        "technical": "The ideal scene gives the human-agent tandem one contract to create, run, move, protect and restore a universe, from a console or an agent’s tools. Hosts carry the operation; a workstation observes and commands. Each universe declares continuity rules and the data to preserve. Exclusive decisions must avoid two instances writing divergent histories; interrupted operations must resume without repeating their effects. Recovery possibilities follow the available infrastructure.",
        "doc": "docs/IDEAL-SCENE.md"
      },
      {
        "id": "meshfs",
        "name": "MeshFS",
        "short": "Keep track of files",
        "title": "Keep a history, without copying everything again.",
        "lead": "Files change, and we would like to find their history without keeping a full copy at every step. MeshFS starts from that idea: organise content into blocks recognised by their digital fingerprints, then reuse those already present. An exciting direction to explore, starting with simple files.",
        "points": [
          [
            "Reuse what already exists",
            "Two versions contain identical blocks? MeshFS can store them once and reuse them. This is deduplication. The space saved depends on the common content."
          ],
          [
            "Keep reference points",
            "A snapshot describes files at a particular time. It helps compare versions and find a state again with a reference to guide you."
          ],
          [
            "Understand by trying",
            "A test folder helps you discover the approach. MeshFS can be used without PodMesh; working across servers adds sharing and synchronisation."
          ]
        ],
        "example": "You develop a folder over several days. Some parts change; others stay identical. MeshFS can reuse the blocks shared across versions, so you can examine changes and find your reference points again.",
        "workflow": [
          "Identify the files, storage in use and available capacity.",
          "Create or inspect a snapshot and keep its reference.",
          "Compare fingerprints and check recovered files. For a copy on another server, also check its presence and readability."
        ],
        "technical": "MeshFS builds on content-addressed storage, or CAS: blocks identified by SHA-256 fingerprints can be reused. A Merkle structure connects files to their blocks and helps identify changes. This work on files complements PodMesh’s work on applications and PMBS’s work on backups. Shared storage, continuity copies and backups answer different needs; we want each to remain understandable.",
        "doc": "crates/meshfs",
        "docLabel": "Read the MeshFS source code"
      },
      {
        "id": "pmbs",
        "name": "PMBS",
        "short": "Back up so you can recover",
        "title": "A backup you know how to use.",
        "lead": "What we want from a backup is to recover our data when we need it. With PMBS — PodMesh Backup Server — we work on that whole path: keeping files, checking their integrity and preparing restoration. The aim is simple: know how to get going again.",
        "points": [
          [
            "Find a backup",
            "Identify the contents kept and the backup point they belong to."
          ],
          [
            "Reuse identical blocks",
            "Backups can share the blocks they have in common. Capacity needs depend on the data and how it changes."
          ],
          [
            "Try recovery",
            "Check the blocks, restore to a test destination and verify that the files and application can be used. Each step brings its own evidence; together, they build confidence."
          ]
        ],
        "example": "Before an update, you keep a backup and try restoring it in a test space. Finding the files, then running the application, provides practical reference points for the day you need them.",
        "workflow": [
          "Identify the backed-up contents, their reference and the items needed for recovery.",
          "Check block presence and integrity.",
          "Restore to an authorised destination, then check data and application operation."
        ],
        "technical": "In the ideal scene, a backup server outside the group of hosts keeps versioned recovery points protected against modification, with a copy at another site. The path back covers files, databases and the elements required for the application to work. Checking fingerprints verifies integrity; the goal also includes actual restoration and checking that the application works. That whole chain is what should make a backup dependable.",
        "doc": "crates/pmbs",
        "docLabel": "Read the PMBS source code"
      },
      {
        "id": "proxmox",
        "name": "Proxmox VE",
        "short": "Connect storage tools",
        "title": "Explore MeshFS in a familiar environment.",
        "lead": "Proxmox is one of the tools we enjoy using. The project plugin connects its storage tools to MeshFS: a way to continue exploring from a familiar interface and see how this approach meets your needs.",
        "points": [
          [
            "Use familiar tools",
            "Use Proxmox storage interfaces for operations supported by the plugin."
          ],
          [
            "Prepare storage",
            "Define its location, participating servers and permitted content types."
          ],
          [
            "Try a test volume",
            "Check creation, writes, reads and recovery before placing an important machine on it."
          ]
        ],
        "example": "You already know Proxmox and want to discover MeshFS. Start with a test volume, write data, read it back and try retrieving it after restoration. The exploration builds on familiar actions.",
        "workflow": [
          "Check the Proxmox, MeshFS and plugin versions.",
          "Prepare storage configuration within the authorised scope.",
          "Test the required operations with actual data and keep the results."
        ],
        "technical": "The plugin connects Proxmox VE’s storage interface to MeshFS through PVE::Storage::Plugin. The intention is to bring storage operations into familiar tools with clear responsibilities. It is developed within the PodMesh project. Storage and application continuity rules remain distinct subjects.",
        "doc": "packaging/proxmox/README-PROXMOX-PLUGIN.md"
      },
      {
        "id": "downloads",
        "name": "Getting started",
        "short": "Choose what to install",
        "title": "Start small, learn by doing.",
        "lead": "A folder, a container or a test backup: one specific use is enough to begin understanding. We invite you to explore a component, observe what it does and get your bearings before going further.",
        "points": [
          [
            "Choose a starting point",
            "Understand containers with PodMesh, explore files with MeshFS or prepare a backup with PMBS."
          ],
          [
            "Read the relevant procedure",
            "Find prerequisites, versions and required components in the documentation."
          ],
          [
            "Run a first trial",
            "Use test data, record what was installed and check the result before extending the use."
          ]
        ],
        "example": "To discover MeshFS, start with a folder whose files you know. Create a snapshot, change a few items and look at what changed. This first trial helps you understand what the tool could bring you.",
        "workflow": [
          "Clarify the desired result and check machine prerequisites.",
          "Prepare installation from an identified source, with its versions and fingerprints.",
          "Report what is installed, what works and which steps remain."
        ],
        "technical": "The project separates application management, command tools, MeshFS storage and the PMBS backup server. Installation guides help choose components, versions and prerequisites. Our direction stays the same: understand what is installed, observe its operations and have a recovery path.",
        "doc": "docs/README.md"
      }
    ],
    "direction": "This guide presents the direction of the project: what we are building and the uses that guide us.",
    "directionLink": "Read the ideal scene"
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
        "short": "Mover y replicar aplicaciones",
        "title": "Una aplicación, más allá de su primer servidor.",
        "lead": "Una aplicación vive en una máquina, pero su ubicación solo es una parte. Tiene servicios, archivos y un estado de ejecución: lo que está haciendo ahora. Con PodMesh y Podman exploramos cómo gestionar ese conjunto, trasladarlo en caliente y preparar una réplica, cuidando lo que le permite funcionar.",
        "points": [
          [
            "Ver qué estás cuidando",
            "Identificar los contenedores, servicios y datos que dan vida a la aplicación. En SHAPER OS, este conjunto se llama universo."
          ],
          [
            "Explorar el traslado en caliente",
            "Capturar un estado de ejecución y reanudarlo en otra máquina: nos interesa lo que hacía la aplicación además de sus archivos. El proceso depende de la configuración."
          ],
          [
            "Preparar una copia para recuperar",
            "La replicación mantiene una copia en otro servidor. Trasladar y replicar una aplicación responden a necesidades distintas que trabajamos en el mismo proyecto."
          ]
        ],
        "example": "Un servidor necesita mantenimiento. Quieres preparar el traslado de tu aplicación a otra máquina, encontrar sus datos y comprobar que funciona antes de continuar. Es una necesidad cotidiana y una buena razón para trabajar con cuidado en el traslado y la recuperación.",
        "workflow": [
          "Observar el estado real de las aplicaciones y los recursos disponibles.",
          "Preparar la operación con las interfaces de la versión instalada y los permisos recibidos.",
          "Comprobar la aplicación y sus datos después, e indicar qué ha funcionado y qué queda pendiente."
        ],
        "technical": "La escena ideal ofrece al tándem persona-agente un mismo contrato para crear, ejecutar, trasladar, proteger y restaurar un universo, desde una consola o las herramientas de un agente. Los hosts llevan el funcionamiento; una estación de trabajo observa y manda. Cada universo declara sus reglas de continuidad y los datos que preservar. Las decisiones exclusivas deben evitar dos instancias que escriban historias distintas; las operaciones interrumpidas deben reanudarse sin repetir efectos. La recuperación sigue la infraestructura disponible.",
        "doc": "docs/IDEAL-SCENE.md"
      },
      {
        "id": "meshfs",
        "name": "MeshFS",
        "short": "Seguir la historia de los archivos",
        "title": "Guardar la historia, sin volver a copiarlo todo.",
        "lead": "Los archivos cambian y nos gustaría encontrar su historia sin guardar una copia completa en cada etapa. MeshFS parte de esta idea: organizar el contenido en bloques reconocidos por sus huellas digitales y reutilizar los que ya existen. Una dirección apasionante para explorar con archivos sencillos.",
        "points": [
          [
            "Reutilizar lo que ya existe",
            "¿Dos versiones contienen bloques idénticos? MeshFS puede guardarlos una sola vez y reutilizarlos. Es la deduplicación. El ahorro depende del contenido común."
          ],
          [
            "Conservar referencias",
            "Una instantánea, o snapshot, describe los archivos en un momento concreto. Ayuda a comparar versiones y encontrar un estado con una referencia que te orienta."
          ],
          [
            "Comprender probando",
            "Una carpeta de prueba ayuda a descubrir la idea. MeshFS puede utilizarse sin PodMesh; trabajar entre servidores añade intercambio y sincronización."
          ]
        ],
        "example": "Una carpeta evoluciona con los días. Algunas partes cambian y otras siguen idénticas. MeshFS puede reutilizar los bloques comunes entre versiones para examinar los cambios y encontrar tus referencias.",
        "workflow": [
          "Identificar los archivos, el almacenamiento y el espacio disponible.",
          "Crear o examinar una instantánea y conservar su referencia.",
          "Comparar las huellas y comprobar los archivos recuperados. Para una copia en otro servidor, comprobar también su presencia y lectura."
        ],
        "technical": "MeshFS parte del almacenamiento direccionado por contenido, o CAS: los bloques identificados por huellas SHA-256 pueden reutilizarse. Una estructura de Merkle relaciona los archivos con sus bloques y ayuda a detectar cambios. Este trabajo sobre los archivos complementa el de PodMesh sobre aplicaciones y el de PMBS sobre copias de seguridad. El almacenamiento compartido, las copias para continuidad y las copias de seguridad responden a necesidades distintas; queremos que cada una siga siendo comprensible.",
        "doc": "crates/meshfs",
        "docLabel": "Ver el código fuente de MeshFS"
      },
      {
        "id": "pmbs",
        "name": "PMBS",
        "short": "Guardar para poder recuperar",
        "title": "Una copia que sabes cómo utilizar.",
        "lead": "Lo que esperamos de una copia de seguridad es recuperar nuestros datos cuando los necesitamos. Con PMBS — PodMesh Backup Server — trabajamos en todo ese camino: conservar archivos, comprobar su integridad y preparar la restauración. El objetivo es sencillo: saber cómo volver a empezar.",
        "points": [
          [
            "Encontrar una copia",
            "Identificar el contenido conservado y el punto de copia al que corresponde."
          ],
          [
            "Reutilizar bloques idénticos",
            "Las copias pueden compartir sus bloques comunes. El espacio necesario depende de los datos y sus cambios."
          ],
          [
            "Probar la recuperación",
            "Comprobar los bloques, restaurar en un destino de prueba y verificar que los archivos y la aplicación son utilizables. Cada etapa aporta su propia prueba y, juntas, ayudan a confiar en el resultado."
          ]
        ],
        "example": "Antes de actualizar, guardas una copia y pruebas su restauración en un espacio de prueba. Recuperar los archivos y después hacer funcionar la aplicación da referencias concretas para cuando las necesites.",
        "workflow": [
          "Identificar el contenido guardado, su referencia y los elementos necesarios para recuperarlo.",
          "Comprobar la presencia e integridad de los bloques.",
          "Restaurar en un destino autorizado y comprobar los datos y el funcionamiento de la aplicación."
        ],
        "technical": "En la escena ideal, un servidor de copias fuera del grupo de máquinas guarda puntos de recuperación versionados, protegidos contra modificaciones, con una copia en otra ubicación. El camino de vuelta cubre archivos, bases de datos y elementos necesarios para que funcione la aplicación. Comprobar huellas verifica la integridad; el objetivo incluye restaurar de verdad y comprobar el funcionamiento. Toda esa cadena debe permitir confiar en la copia.",
        "doc": "crates/pmbs",
        "docLabel": "Ver el código fuente de PMBS"
      },
      {
        "id": "proxmox",
        "name": "Proxmox VE",
        "short": "Conectar el almacenamiento",
        "title": "Explorar MeshFS en un entorno familiar.",
        "lead": "Proxmox es una de las herramientas que disfrutamos usando. El plugin del proyecto conecta su almacenamiento con MeshFS: una forma de seguir explorando desde una interfaz conocida y ver cómo responde a tus necesidades.",
        "points": [
          [
            "Usar herramientas conocidas",
            "Pasar por las interfaces de almacenamiento de Proxmox para las operaciones compatibles con el plugin."
          ],
          [
            "Preparar el almacenamiento",
            "Definir su ubicación, los servidores participantes y los tipos de contenido permitidos."
          ],
          [
            "Probar con un volumen de prueba",
            "Comprobar creación, escritura, lectura y recuperación antes de alojar una máquina importante."
          ]
        ],
        "example": "Ya conoces Proxmox y quieres descubrir MeshFS. Empiezas con un volumen de prueba, escribes datos, los lees e intentas recuperarlos después de restaurar. La exploración se apoya en acciones conocidas.",
        "workflow": [
          "Comprobar las versiones de Proxmox, MeshFS y el plugin.",
          "Preparar la configuración de almacenamiento dentro del alcance autorizado.",
          "Probar las operaciones necesarias con datos reales y conservar los resultados."
        ],
        "technical": "El plugin conecta la interfaz de almacenamiento de Proxmox VE con MeshFS mediante PVE::Storage::Plugin. La intención es llevar las operaciones de almacenamiento a herramientas familiares con responsabilidades claras. Se desarrolla dentro del proyecto PodMesh. El almacenamiento y las reglas de continuidad de la aplicación siguen siendo temas distintos.",
        "doc": "packaging/proxmox/README-PROXMOX-PLUGIN.md"
      },
      {
        "id": "downloads",
        "name": "Primeros pasos",
        "short": "Elegir qué instalar",
        "title": "Empezar poco a poco, aprender haciendo.",
        "lead": "Una carpeta, un contenedor o una copia de prueba: un uso concreto basta para empezar a comprender. Te invitamos a explorar un componente, observar qué hace y orientarte antes de avanzar.",
        "points": [
          [
            "Elegir un punto de partida",
            "Comprender los contenedores con PodMesh, explorar archivos con MeshFS o preparar una copia con PMBS."
          ],
          [
            "Leer el procedimiento",
            "Consultar los requisitos, las versiones y los componentes necesarios en la documentación."
          ],
          [
            "Hacer una primera prueba",
            "Usar datos de prueba, anotar lo instalado y comprobar el resultado antes de ampliar el uso."
          ]
        ],
        "example": "Para descubrir MeshFS, empieza con una carpeta cuyos archivos conoces. Crea una instantánea, cambia algunos elementos y observa qué ha cambiado. Esta primera prueba ayuda a comprender qué te puede aportar la herramienta.",
        "workflow": [
          "Aclarar el resultado esperado y comprobar los requisitos de la máquina.",
          "Preparar la instalación desde una fuente identificada, con sus versiones y huellas.",
          "Indicar qué está instalado, qué funciona y qué pasos faltan."
        ],
        "technical": "El proyecto separa la gestión de aplicaciones, las herramientas de comandos, el almacenamiento MeshFS y el servidor de copias PMBS. Las guías de instalación ayudan a elegir componentes, versiones y requisitos. Nuestro rumbo es el mismo: comprender lo instalado, observar sus operaciones y disponer de un camino de recuperación.",
        "doc": "docs/README.md"
      }
    ],
    "direction": "Esta presentación muestra el rumbo del proyecto: lo que construimos y los usos que nos guían.",
    "directionLink": "Leer la escena ideal"
  }
}
