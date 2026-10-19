"""Cloudflare Access: every request must carry a valid Access token for an allowed email."""

import asyncio

import jwt


class Access:
    def __init__(self, team: str, aud: str, emails: list[str]):
        if not (team and aud and emails):
            raise SystemExit("seimei: [access] needs team, aud and emails in the config (or run with --dev)")
        self.issuer = f"https://{team}.cloudflareaccess.com"
        self.aud = aud
        self.emails = {e.lower() for e in emails}
        self.jwks = jwt.PyJWKClient(f"{self.issuer}/cdn-cgi/access/certs", cache_keys=True, lifespan=3600)

    async def email(self, request) -> str | None:
        """The verified, allowed email behind a request, or None."""
        token = request.headers.get("Cf-Access-Jwt-Assertion")
        if not token:
            return None
        try:
            key = await asyncio.to_thread(self.jwks.get_signing_key_from_jwt, token)
            claims = jwt.decode(token, key.key, algorithms=["RS256"], audience=self.aud, issuer=self.issuer,
                                options={"require": ["exp", "iat", "aud", "iss"]})
        except jwt.PyJWTError:
            return None
        email = str(claims.get("email", "")).lower()
        return email if email in self.emails else None
