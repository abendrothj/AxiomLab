FROM python:3.12-slim

WORKDIR /app

# Install dependencies first (cache layer)
COPY pyproject.toml .
RUN pip install --no-cache-dir sila2 typer cryptography

# Copy source
COPY . .
RUN pip install --no-cache-dir -e .

EXPOSE 50052

CMD ["python", "-m", "axiomlab_mock", "--insecure", "-p", "50052"]
