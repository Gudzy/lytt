FROM python:3.12-slim

# camoufox bundles its own playwright integration — no separate playwright install needed
RUN pip install --no-cache-dir \
    "camoufox[geoip]" \
    azure-identity \
    azure-mgmt-appcontainers

# Install Firefox runtime system dependencies (GTK3, NSS, etc.)
RUN python -m playwright install-deps firefox

# Downloads the hardened Firefox binary into the image
RUN python -m camoufox fetch

COPY rotate.py /rotate.py

ENTRYPOINT ["python", "/rotate.py"]
