# The pytest quarantine sample. Build with the samples/quarantine/ directory as
# the context (samples/quarantine/test.sh does this): the COPY path is pytest/.
FROM python:3.12-slim
RUN pip install --no-cache-dir pytest==9.2.0
WORKDIR /sample
COPY pytest/ /sample/
CMD ["python", "-m", "pytest", "-p", "pytester", "-q", "test_flakeradar_quarantine.py"]
