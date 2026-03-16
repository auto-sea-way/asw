# Deployment Guide

auto-sea-way serves a maritime routing API over HTTP. This guide covers three deployment options.

## Docker Compose

### Full image (zero-config, ~870 MB)

The graph is baked into the image — no downloads or volumes needed.

```yaml
services:
  asw:
    image: ghcr.io/auto-sea-way/asw:0.1.0-full
    ports:
      - "3000:3000"
    environment:
      - ASW_API_KEY=${ASW_API_KEY}
    healthcheck:
      test: ["CMD", "/usr/local/bin/asw", "healthcheck"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 30s
```

### Slim image (auto-download, ~25 MB)

The graph is downloaded on first start and cached in a named volume.

```yaml
services:
  asw:
    image: ghcr.io/auto-sea-way/asw:0.1.0
    ports:
      - "3000:3000"
    environment:
      - ASW_API_KEY=${ASW_API_KEY}
      - ASW_GRAPH_URL=https://github.com/auto-sea-way/asw/releases/download/v0.1.0/asw.graph
    volumes:
      - asw-data:/data
    healthcheck:
      test: ["CMD", "/usr/local/bin/asw", "healthcheck"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 120s  # graph download on first start; adjust for network speed
volumes:
  asw-data:
```

## Kubernetes

### Deployment (full image)

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: asw
spec:
  replicas: 1
  selector:
    matchLabels:
      app: asw
  template:
    metadata:
      labels:
        app: asw
    spec:
      containers:
        - name: asw
          image: ghcr.io/auto-sea-way/asw:0.1.0-full
          ports:
            - containerPort: 3000
          env:
            - name: ASW_API_KEY
              valueFrom:
                secretKeyRef:
                  name: asw-secrets
                  key: api-key
          readinessProbe:
            httpGet:
              path: /ready
              port: 3000
            periodSeconds: 5
            failureThreshold: 3
          livenessProbe:
            httpGet:
              path: /health
              port: 3000
            periodSeconds: 10
          resources:
            requests:
              memory: "2Gi"
              cpu: "500m"
            limits:
              memory: "4Gi"
```

### Deployment (slim image with PVC)

```yaml
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: asw-data
spec:
  accessModes: [ReadWriteOnce]
  resources:
    requests:
      storage: 2Gi
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: asw
spec:
  replicas: 1
  selector:
    matchLabels:
      app: asw
  template:
    metadata:
      labels:
        app: asw
    spec:
      containers:
        - name: asw
          image: ghcr.io/auto-sea-way/asw:0.1.0
          ports:
            - containerPort: 3000
          env:
            - name: ASW_API_KEY
              valueFrom:
                secretKeyRef:
                  name: asw-secrets
                  key: api-key
            - name: ASW_GRAPH_URL
              value: https://github.com/auto-sea-way/asw/releases/download/v0.1.0/asw.graph
          volumeMounts:
            - name: data
              mountPath: /data
          readinessProbe:
            httpGet:
              path: /ready
              port: 3000
            initialDelaySeconds: 30
            periodSeconds: 5
          livenessProbe:
            httpGet:
              path: /health
              port: 3000
            periodSeconds: 10
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: asw-data
```

### Service

```yaml
apiVersion: v1
kind: Service
metadata:
  name: asw
spec:
  selector:
    app: asw
  ports:
    - port: 3000
      targetPort: 3000
  type: ClusterIP
```

### Ingress (optional)

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: asw
spec:
  rules:
    - host: routing.example.com
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: asw
                port:
                  number: 3000
```

## Bare-metal

### Download

```bash
# Download binary (pick your platform)
gh release download v0.1.0 --repo auto-sea-way/asw --pattern 'asw-linux-amd64'
chmod +x asw-linux-amd64
sudo mv asw-linux-amd64 /usr/local/bin/asw

# Download graph
gh release download v0.1.0 --repo auto-sea-way/asw --pattern 'asw.graph'
sudo mkdir -p /var/lib/asw
sudo mv asw.graph /var/lib/asw/
```

### Run

```bash
ASW_API_KEY=your-secret asw serve --graph /var/lib/asw/asw.graph --port 3000
```

### Systemd service

```ini
# /etc/systemd/system/asw.service
[Unit]
Description=auto-sea-way maritime routing
After=network.target

[Service]
ExecStart=/usr/local/bin/asw serve --graph /var/lib/asw/asw.graph
Restart=on-failure
RestartSec=5
Environment=ASW_PORT=3000
Environment=ASW_HOST=0.0.0.0
Environment=ASW_API_KEY=your-secret-here

[Install]
WantedBy=multi-user.target
```

Enable and start:

```bash
sudo systemctl enable --now asw
```
