FROM python:3.12-slim

# Playwright + Camoufox + Azure SDK
RUN pip install --no-cache-dir \
    playwright \
    playwright-camoufox \
    "camoufox[geoip]" \
    azure-identity \
    azure-mgmt-appcontainers

# Camoufox downloads its own hardened Firefox binary
RUN python -m camoufox fetch

COPY rotate.py /rotate.py

ENTRYPOINT ["python", "/rotate.py"]
