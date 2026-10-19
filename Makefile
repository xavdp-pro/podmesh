.PHONY: all dev dev-check prod test check clean health packages

all: check

# Mode Développement : Lance MeshFS, PMBS et le Showcase en local isolé
dev:
	@bash scripts/dev.sh

# Mode Dev Auto-Check : Teste le démarrage et l'arrêt propre sans intervention
dev-check:
	@bash scripts/dev.sh --check

# Mode Production : Compile en release LTO, génère tous les paquets Debian (.deb) et bundle web
prod:
	@bash scripts/prod.sh

# Génère l'ensemble des paquets Debian
packages:
	@bash packaging/build-all-debs.sh

# Génère les 2 repositories Debian APT (Canal DEV et Canal PROD)
apt-all:
	@bash packaging/generate-apt-repos.sh all

apt-dev:
	@bash packaging/generate-apt-repos.sh dev

apt-prod:
	@bash packaging/generate-apt-repos.sh prod

# Exécute l'ensemble des tests unitaires du workspace Rust
test:
	@cargo test --workspace

# Vérification rapide de syntaxe et types
check:
	@cargo check --workspace

# Audit de santé des services (HTTP + Sockets)
health:
	@bash scripts/healthcheck.sh

# Nettoyage des binaires et artefacts
clean:
	@cargo clean
	@rm -rf data/dev/* dist/*
