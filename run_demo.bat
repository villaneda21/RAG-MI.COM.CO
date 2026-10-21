@echo off
cd /d "%~dp0"
docker compose up --build -d
echo Cliente: http://127.0.0.1:8000/client
echo CRM:     http://127.0.0.1:8000/crm
echo Token de demostracion: el valor de CRM_SECRET_TOKEN o demo-crm-token
