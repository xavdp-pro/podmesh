// Reading aid shared with xavdp.pro; definitions extended for PodMesh.
export const GLOSSARY = {
  "Shaper OS": {
    "label": {
      "fr": "Shaper OS",
      "en": "Shaper OS",
      "es": "Shaper OS"
    },
    "match": [
      "Shaper OS",
      "ShaperOS"
    ],
    "definition": {
      "fr": "Un cadre d’organisation pour le travail entre humains et agents IA : intentions, responsabilités, mandats, vérification et correction. Les logiciels métier et leur infrastructure en sont des réalisations.",
      "en": "An organisational framework for work between humans and AI agents: intentions, responsibilities, mandates, verification and correction. Business software and infrastructure are implementations of it.",
      "es": "Un marco de organización para el trabajo entre humanos y agentes de IA: intenciones, responsabilidades, mandatos, verificación y corrección. El software de negocio y su infraestructura son realizaciones de este marco."
    }
  },
  "agent IA": {
    "label": {
      "fr": "agent IA",
      "en": "AI agent",
      "es": "agente IA"
    },
    "match": [
      "agents IA",
      "agent IA",
      "agents d’IA",
      "agent d’IA",
      "AI agents",
      "AI agent",
      "agentes IA",
      "agente IA",
      "agente de IA",
      "agentes de IA"
    ],
    "definition": {
      "fr": "Un programme d’IA qui fait un travail (classer, relancer, écrire, piloter un outil) au lieu de seulement discuter. Ce n’est pas un chatbot.",
      "en": "An AI program that does a job (filing, following up, writing, driving a tool) instead of only chatting. It is not a chatbot.",
      "es": "Un programa de IA que hace un trabajo (clasificar, reclamar, escribir, pilotar una herramienta) en vez de solo charlar. No es un chatbot."
    }
  },
  "Linux": {
    "label": {
      "fr": "Linux",
      "en": "Linux",
      "es": "Linux"
    },
    "match": [
      "Linux"
    ],
    "definition": {
      "fr": "Un système d’exploitation libre, très utilisé sur les serveurs. C’est sur Linux que Shaper OS tourne, sur un environnement maîtrisé et inspectable.",
      "en": "A free operating system, widely used on servers. Shaper OS runs on Linux, on a controlled and inspectable environment.",
      "es": "Un sistema operativo libre, muy usado en servidores. Shaper OS corre en Linux, en un entorno controlado e inspeccionable."
    }
  },
  "Podman": {
    "label": {
      "fr": "Podman",
      "en": "Podman",
      "es": "Podman"
    },
    "match": [
      "Podman"
    ],
    "definition": {
      "fr": "L’outil qui isole chaque morceau du système dans un « conteneur », pour qu’une panne ou une mise à jour ne fasse pas tout tomber.",
      "en": "The tool that isolates each piece of the system in a “container”, so that a failure or an update does not take everything down.",
      "es": "La herramienta que aísla cada pieza del sistema en un «contenedor», para que un fallo o una actualización no tumbe todo."
    }
  },
  "conteneur": {
    "label": {
      "fr": "conteneur",
      "en": "container",
      "es": "contenedor"
    },
    "match": [
      "conteneurs",
      "conteneur",
      "conteneurisée",
      "conteneurisé",
      "containers",
      "container",
      "containerised",
      "containerized",
      "contenedores",
      "contenedor",
      "contenerizada",
      "contenerizado"
    ],
    "definition": {
      "fr": "Une boîte étanche autour d’un programme : il tourne avec tout ce dont il a besoin, sans polluer le reste de la machine. Ça isole, et ça permet de relever une pièce sans tout reconstruire.",
      "en": "A sealed box around a program: it runs with everything it needs, without polluting the rest of the machine. It isolates, and it lets us raise one piece without rebuilding everything.",
      "es": "Una caja estanca alrededor de un programa: corre con todo lo que necesita, sin ensuciar el resto de la máquina. Aísla, y permite levantar una pieza sin reconstruir todo."
    }
  },
  "GitHub": {
    "label": {
      "fr": "GitHub",
      "en": "GitHub",
      "es": "GitHub"
    },
    "match": [
      "GitHub"
    ],
    "definition": {
      "fr": "Le lieu public où le code est publié et versionné. Le kit d’installation de Shaper OS y est lisible : ce n’est pas une brochure, c’est la loi du système.",
      "en": "The public place where code is published and versioned. The Shaper OS install kit is readable there: it is not a brochure, it is the system’s law.",
      "es": "El lugar público donde se publica y versiona el código. El kit de instalación de Shaper OS se puede leer ahí: no es un folleto, es la ley del sistema."
    }
  },
  "API": {
    "label": {
      "fr": "API",
      "en": "API",
      "es": "API"
    },
    "match": [
      "APIs",
      "API"
    ],
    "definition": {
      "fr": "La porte d’entrée par laquelle deux logiciels se parlent directement, sans passer par un écran. C’est ce qui permet de relier des outils qui s’ignorent.",
      "en": "The doorway through which two pieces of software talk directly, without going through a screen. It is what connects tools that otherwise ignore each other.",
      "es": "La puerta por la que dos programas se hablan directamente, sin pasar por una pantalla. Es lo que conecta herramientas que se ignoran."
    }
  },
  "snapshot": {
    "match": [
      "snapshot",
      "snapshots",
      "instantané",
      "instantanés",
      "instantánea",
      "instantáneas"
    ],
    "label": {
      "fr": "Instantané (snapshot)",
      "en": "Snapshot",
      "es": "Instantánea (snapshot)"
    },
    "definition": {
      "fr": "Une référence à l’état des fichiers à un moment donné. Elle permet de retrouver cet état ; si elle dépend du même disque, elle ne remplace pas une copie de sauvegarde sur un autre support.",
      "en": "A reference to the state of files at a particular time. It helps find that state again; if it depends on the same disk, it does not replace a backup copy on another storage device.",
      "es": "Una referencia al estado de los archivos en un momento concreto. Permite encontrar ese estado; si depende del mismo disco, no sustituye una copia en otro soporte."
    }
  },
  "déduplication": {
    "match": [
      "déduplication",
      "deduplication",
      "deduplicación"
    ],
    "label": {
      "fr": "Déduplication",
      "en": "Deduplication",
      "es": "Deduplicación"
    },
    "definition": {
      "fr": "Conserver une seule fois les blocs identiques et les réutiliser dans plusieurs fichiers ou versions. Le gain de place dépend de la quantité de contenu commun.",
      "en": "Storing identical blocks once and reusing them across files or versions. The space saved depends on how much content is shared.",
      "es": "Guardar una sola vez los bloques idénticos y reutilizarlos entre archivos o versiones. El ahorro depende de cuánto contenido comparten."
    }
  },
  "empreinte": {
    "match": [
      "empreinte numérique",
      "empreintes numériques",
      "empreinte",
      "empreintes",
      "digital fingerprint",
      "fingerprints",
      "fingerprint",
      "huella digital",
      "huellas",
      "huella"
    ],
    "label": {
      "fr": "Empreinte numérique",
      "en": "Digital fingerprint",
      "es": "Huella digital"
    },
    "definition": {
      "fr": "Une valeur calculée à partir d’un contenu. La recalculer et la comparer aide à vérifier si les octets ont changé ; ce n’est pas un chiffrement.",
      "en": "A value calculated from content. Recalculating and comparing it helps check whether the bytes changed; it is not encryption.",
      "es": "Un valor calculado a partir de un contenido. Volver a calcularlo y compararlo permite comprobar si los bytes cambiaron; no es un cifrado."
    }
  },
  "CAS": {
    "match": [
      "CAS"
    ],
    "label": {
      "fr": "CAS — stockage adressé par le contenu",
      "en": "CAS — content-addressed storage",
      "es": "CAS — almacenamiento por contenido"
    },
    "definition": {
      "fr": "Une méthode de rangement qui utilise l’empreinte d’un bloc pour l’identifier. Deux blocs au contenu identique peuvent ainsi partager la même référence.",
      "en": "A way to store blocks using their content fingerprints as identifiers. Blocks with identical contents can share the same reference.",
      "es": "Una forma de guardar bloques usando la huella de su contenido como identificador. Los bloques idénticos pueden compartir una referencia."
    }
  },
  "SHA-256": {
    "match": [
      "SHA-256"
    ],
    "label": {
      "fr": "SHA-256",
      "en": "SHA-256",
      "es": "SHA-256"
    },
    "definition": {
      "fr": "La méthode de calcul des empreintes utilisée ici. Elle sert à comparer le contenu des blocs et à détecter une différence lors d’une vérification.",
      "en": "The fingerprint calculation method used here. It helps compare block contents and detect a difference during a check.",
      "es": "El método utilizado aquí para calcular las huellas. Ayuda a comparar el contenido de los bloques y detectar diferencias al comprobarlos."
    }
  },
  "Merkle": {
    "match": [
      "Merkle"
    ],
    "label": {
      "fr": "Structure de Merkle",
      "en": "Merkle structure",
      "es": "Estructura de Merkle"
    },
    "definition": {
      "fr": "Une structure qui relie les empreintes des fichiers et de leurs blocs. Quand un contenu change, les empreintes associées changent aussi : cela permet de repérer des différences.",
      "en": "A structure connecting fingerprints of files and their blocks. When content changes, the related fingerprints change too, helping identify differences.",
      "es": "Una estructura que relaciona las huellas de archivos y bloques. Cuando cambia un contenido, también cambian sus huellas asociadas, lo que ayuda a detectar diferencias."
    }
  },
  "Rust": {
    "match": [
      "Rust"
    ],
    "label": {
      "fr": "Rust",
      "en": "Rust",
      "es": "Rust"
    },
    "definition": {
      "fr": "Le langage de programmation employé pour ces moteurs. Il permet de travailler près du système tout en contrôlant de nombreux risques liés à la mémoire.",
      "en": "The programming language used for these engines. It supports low-level system work while controlling many memory-related risks.",
      "es": "El lenguaje de programación de estos motores. Permite trabajar cerca del sistema controlando muchos riesgos relacionados con la memoria."
    }
  },
  "Mio": {
    "match": [
      "Mio",
      "MiB"
    ],
    "label": {
      "fr": "Mio — mébioctet",
      "en": "MiB — mebibyte",
      "es": "MiB — mebibyte"
    },
    "definition": {
      "fr": "Une unité de taille informatique. Un bloc de 4 Mio représente 4 194 304 octets, soit un peu plus de quatre millions d’octets.",
      "en": "A unit of digital size. A 4 MiB block contains 4,194,304 bytes, a little over four million bytes.",
      "es": "Una unidad de tamaño informático. Un bloque de 4 MiB contiene 4 194 304 bytes, algo más de cuatro millones."
    }
  },
  "APT": {
    "match": [
      "APT"
    ],
    "label": {
      "fr": "APT",
      "en": "APT",
      "es": "APT"
    },
    "definition": {
      "fr": "L’outil qui installe et met à jour les paquets sur Debian. Un dépôt APT est la source dans laquelle il va les chercher.",
      "en": "The tool that installs and updates packages on Debian. An APT repository is the source it retrieves them from.",
      "es": "La herramienta que instala y actualiza paquetes en Debian. Un repositorio APT es la fuente de la que los obtiene."
    }
  },
  "CLI": {
    "match": [
      "CLI"
    ],
    "label": {
      "fr": "CLI — ligne de commande",
      "en": "CLI — command-line interface",
      "es": "CLI — línea de comandos"
    },
    "definition": {
      "fr": "Une façon de piloter un outil en tapant des commandes. Une personne, un script ou un agent IA peut l’utiliser.",
      "en": "A way to control a tool by entering commands. A person, a script or an AI agent can use it.",
      "es": "Una forma de manejar una herramienta escribiendo comandos. Puede utilizarla una persona, un script o un agente de IA."
    }
  },
  "PMBS": {
    "match": [
      "PMBS"
    ],
    "label": {
      "fr": "PMBS — PodMesh Backup Server",
      "en": "PMBS — PodMesh Backup Server",
      "es": "PMBS — PodMesh Backup Server"
    },
    "definition": {
      "fr": "Le composant du projet consacré aux sauvegardes. Il conserve le contenu et les références nécessaires pour le retrouver.",
      "en": "The project component dedicated to backups. It keeps content and the references needed to retrieve it.",
      "es": "El componente del proyecto dedicado a las copias de seguridad. Guarda el contenido y las referencias para recuperarlo."
    }
  },
  "plugin": {
    "match": [
      "plugin",
      "plugins"
    ],
    "label": {
      "fr": "Plugin",
      "en": "Plugin",
      "es": "Plugin"
    },
    "definition": {
      "fr": "Un module qui ajoute une possibilité à un logiciel existant. Ici, il fait le lien entre les outils de stockage de Proxmox et MeshFS.",
      "en": "A module adding a capability to existing software. Here it connects Proxmox storage tools to MeshFS.",
      "es": "Un módulo que añade una función a un programa existente. Aquí conecta las herramientas de almacenamiento de Proxmox con MeshFS."
    }
  },
  "univers": {
    "match": [
      "univers",
      "universe",
      "universes",
      "universo",
      "universos"
    ],
    "label": {
      "fr": "Univers SHAPER OS",
      "en": "SHAPER OS universe",
      "es": "Universo SHAPER OS"
    },
    "definition": {
      "fr": "Un ensemble organisé autour d’une application, avec ses services et ses données. Cette notion aide à définir ce qu’on gère ensemble et qui en est responsable.",
      "en": "A whole organised around an application, its services and data. It helps define what is managed together and who is responsible.",
      "es": "Un conjunto organizado alrededor de una aplicación, sus servicios y sus datos. Ayuda a definir qué se gestiona junto y quién es responsable."
    }
  }
}

GLOSSARY["live-move"] = {
  "match": [
    "déplacement à chaud",
    "déplacer une application à chaud",
    "à chaud",
    "live movement",
    "running state",
    "traslado en caliente",
    "en caliente"
  ],
  "label": {
    "fr": "Déplacement à chaud",
    "en": "Live movement",
    "es": "Traslado en caliente"
  },
  "definition": {
    "fr": "Déplacer une application en tenant compte de son état d’exécution, par exemple sa mémoire et ses processus. La capture et la reprise peuvent comporter une pause ; le déroulement dépend de la configuration.",
    "en": "Moving an application while accounting for its running state, such as memory and processes. Capturing and resuming may involve a pause; the behaviour depends on the configuration.",
    "es": "Trasladar una aplicación teniendo en cuenta su estado de ejecución, como memoria y procesos. La captura y reanudación pueden incluir una pausa; el funcionamiento depende de la configuración."
  }
}

GLOSSARY["replication"] = {
  "match": [
    "réplication",
    "répliquer",
    "replication",
    "replicate",
    "replicación",
    "replicar"
  ],
  "label": {
    "fr": "Réplication",
    "en": "Replication",
    "es": "Replicación"
  },
  "definition": {
    "fr": "Maintenir une copie des données ou de l’application sur un autre serveur. Pour l’utiliser en reprise, il faut savoir ce qui est synchronisé et comment activer la copie.",
    "en": "Maintaining a copy of data or an application on another server. Using it for recovery requires knowing what is synchronised and how to activate the copy.",
    "es": "Mantener una copia de los datos o de la aplicación en otro servidor. Usarla para recuperar el servicio requiere saber qué se sincroniza y cómo activar la copia."
  }
}

GLOSSARY["geek"] = {
  "match": [
    "geek"
  ],
  "label": {
    "fr": "Geek",
    "en": "Geek",
    "es": "Geek"
  },
  "definition": {
    "fr": "Une personne passionnée par la technique et ses possibilités. Ici, le mot exprime notre plaisir de construire et de comprendre ; aucune connaissance préalable n’est nécessaire pour découvrir le projet.",
    "en": "Someone enthusiastic about technology and what it makes possible. Here the word expresses our enjoyment of building and understanding; no prior knowledge is needed to explore the project.",
    "es": "Una persona apasionada por la tecnología y sus posibilidades. Aquí expresa el placer de construir y comprender; no necesitas conocimientos previos para descubrir el proyecto."
  }
}

GLOSSARY.Astra = {
  match: ['Astra'],
  label: { fr: 'Astra', en: 'Astra', es: 'Astra' },
  definition: {
    fr: 'L’agent IA avec lequel cette exploration a commencé. Les échanges entre l’humain et l’agent ont permis de transformer une découverte en pistes de travail, puis en outils.',
    en: 'The AI agent alongside which this exploration began. Exchanges between the human and the agent helped turn a discovery into directions to explore, then tools.',
    es: 'El agente de IA con el que comenzó esta exploración. Los intercambios entre la persona y el agente ayudaron a transformar un descubrimiento en líneas de trabajo y después en herramientas.',
  },
}

// Match longer expressions before their shorter forms.
const ALL_MATCHES = Object.entries(GLOSSARY)
  .flatMap(([key, entry]) => entry.match.map((needle) => ({ key, needle })))
  .sort((a, b) => b.needle.length - a.needle.length)

function escapeRegex(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

/**
 * Unicode-aware boundaries: "vibe-codé" and "l'OS" match, "API" does not
 * match inside a longer word. Lookbehind/lookahead replace \\b, which breaks
 * on hyphens and on letters like é.
 */
export const GLOSSARY_PATTERN = new RegExp(
  `(?<![\\p{L}\\p{N}_])(${ALL_MATCHES.map((m) => escapeRegex(m.needle)).join('|')})(?![\\p{L}\\p{N}_])`,
  'giu',
)

/** Resolve a matched string back to its glossary entry, or null. */
export function lookup(text) {
  const needle = text.toLowerCase()
  const found = ALL_MATCHES.find((m) => m.needle.toLowerCase() === needle)
  return found ? { key: found.key, ...GLOSSARY[found.key] } : null
}
