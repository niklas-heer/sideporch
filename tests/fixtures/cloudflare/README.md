A 2048-bit RSA key used only by `tests/cloudflare.rs` to sign fake
Cloudflare Access assertions: `key.pk8` (PKCS#8 DER) and its public half
as a JWK in `jwk.json`. It protects nothing. To make a new one:

    openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out key.pem
    openssl pkcs8 -topk8 -nocrypt -in key.pem -outform DER -out key.pk8

and write `n` and `e` from `openssl rsa -inform DER -in key.pk8 -noout -text`
into `jwk.json` as unpadded base64url.
