# Team Configuration

- **Team key:** TRA
- **Team name:** Trakkt
- **GitHub repo:** kyomi-ai/trakkt

## Versioning

- **Method:** date-based
- **Tag format:** vYYYY.MM.DD.N (e.g. v2026.05.19.1, second release same day: v2026.05.19.2)
- **Version source:** git tag (not Cargo.toml)

## Release

- **Pipelines:** release.yml (runs docker, deploy-notify, release jobs)
- **Production URL:** trakkt.app
- **Private deploy repo:** kyomi-ai/trakkt-cloud (triggered via repository_dispatch)
- **Post-release:** Docker image pushed to ghcr.io/kyomi-ai/trakkt, production deployment automatic once deploy-notify shows green

## Worktree

- **Port base:** 3200
- **Reserved port:** 3100 (global dev instance)
- **Port formula:** 3200 + (ticket_number % 800)
