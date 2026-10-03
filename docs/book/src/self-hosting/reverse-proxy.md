# Reverse Proxy

In production, Trakkt should sit behind a reverse proxy that handles TLS termination, domain routing, and WebSocket upgrades. Trakkt itself serves plain HTTP on port 8003 (configurable via `PORT`) and expects the proxy to handle HTTPS.

## WebSocket Support

Trakkt uses WebSockets for real-time sync between clients. Your reverse proxy **must** forward the `Upgrade` and `Connection` headers so WebSocket connections can be established. Without this, the application will fall back to polling or fail to deliver live updates.

## Nginx

### Basic Configuration

```nginx
upstream trakkt {
    server 127.0.0.1:8003;
}

server {
    listen 80;
    server_name trakkt.example.com;
    return 301 https://$host$request_uri;
}

server {
    listen 443 ssl http2;
    server_name trakkt.example.com;

    ssl_certificate     /etc/letsencrypt/live/trakkt.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/trakkt.example.com/privkey.pem;

    location / {
        proxy_pass http://trakkt;

        # Standard proxy headers
        proxy_set_header Host              $host;
        proxy_set_header X-Real-IP         $remote_addr;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # WebSocket support
        proxy_http_version 1.1;
        proxy_set_header Upgrade    $http_upgrade;
        proxy_set_header Connection "upgrade";

        # Timeouts — keep WebSocket connections alive
        proxy_read_timeout 86400s;
        proxy_send_timeout 86400s;
    }
}
```

### Key Directives

| Directive | Purpose |
|-----------|---------|
| `proxy_http_version 1.1` | Required for WebSocket upgrades (HTTP/1.0 does not support `Upgrade`) |
| `proxy_set_header Upgrade` | Passes the WebSocket upgrade request to Trakkt |
| `proxy_set_header Connection "upgrade"` | Tells nginx to switch protocols |
| `proxy_read_timeout 86400s` | Prevents nginx from closing idle WebSocket connections (default is 60s) |

### Docker Compose

If Trakkt runs in Docker Compose alongside nginx, replace the upstream address with the Docker service name:

```nginx
upstream trakkt {
    server trakkt:8003;
}
```

## Caddy

Caddy provides automatic HTTPS via Let's Encrypt with minimal configuration.

### Caddyfile

```
trakkt.example.com {
    reverse_proxy localhost:8003
}
```

Caddy handles WebSocket upgrades, TLS certificates, HTTP-to-HTTPS redirects, and proxy headers automatically. No additional configuration is needed.

### Docker Compose

When running alongside Trakkt in Docker Compose, point to the service name:

```
trakkt.example.com {
    reverse_proxy trakkt:8003
}
```

## Environment Variables

When running behind a reverse proxy, set `BASE_URL` and `FRONTEND_URL` to the public-facing URL so Trakkt constructs correct OAuth redirect URIs, passkey origins, and link URLs:

```bash
BASE_URL=https://trakkt.example.com
FRONTEND_URL=https://trakkt.example.com
WEBAUTHN_RP_ID=trakkt.example.com
```

If `WEBAUTHN_RP_ID` is not set, Trakkt extracts it from `FRONTEND_URL`. Set it explicitly when the domain differs from what the browser sees (e.g., behind a load balancer with a different internal hostname).

See the [Configuration](../getting-started/configuration.md) page for all available environment variables.
