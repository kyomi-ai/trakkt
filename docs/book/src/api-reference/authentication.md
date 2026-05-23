# Authentication

All Trakkt API endpoints require authentication via Bearer tokens. The API uses JWT (JSON Web Tokens) signed with HS256.

## Obtaining a Token

### Via the login API

Authenticate with your email and password to receive a JWT access token:

```bash
curl -X POST https://your-trakkt-instance.com/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email": "you@example.com", "password": "your-password"}'
```

The response includes an access token:

```json
{
  "access_token": "eyJhbGciOiJIUzI1NiIs...",
  "token_type": "Bearer"
}
```

### Via OAuth (SaaS mode)

When Google OAuth is configured, you can authenticate through the browser-based OAuth flow. The resulting session cookie is exchanged for a JWT token.

## Using the Token

Include the token in the `Authorization` header of every API request:

```bash
curl https://your-trakkt-instance.com/api/v1/issues \
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiIs..."
```

## Scopes

Each API operation requires a specific scope. Scopes follow the pattern `resource:action`:

| Scope | Description |
|-------|-------------|
| `issues:read` | Read issues, comments, activities, relations, and GitHub links |
| `issues:write` | Create, update, and delete issues; add comments and relations |
| `teams:read` | List teams |
| `teams:write` | Update team settings |
| `labels:read` | List labels |
| `labels:write` | Create labels |
| `projects:read` | List and get projects and milestones |
| `projects:write` | Create, update, and delete projects and milestones |
| `statuses:read` | List statuses |
| `attachments:read` | List and download attachments |
| `attachments:write` | Upload, delete, attach, and detach attachments |

## Token Expiry

JWT tokens expire after a configured duration. When a token expires, you will receive a `401 Unauthorized` response. Re-authenticate to obtain a new token.

## Security Recommendations

- Store tokens securely and never expose them in client-side code or version control.
- Use HTTPS in production to prevent token interception.
- Rotate the `JWT_SECRET_KEY` periodically and invalidate old tokens.
- In self-hosted deployments, configure the `WEBAUTHN_RP_ID` to match your domain for passkey security.
