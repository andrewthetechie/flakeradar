FROM node:24-slim
# The Vitest / Jest quarantine sample. Build with the samples/quarantine/
# directory as the context (samples/quarantine/test.sh does this): every COPY
# path is prefixed with vitest-jest/.
WORKDIR /sample
# The dependency layer comes first so an edit to the sample's own files does not
# reinstall: example/ resolves vitest, jest and jest-junit from /sample/node_modules.
COPY vitest-jest/package.json /sample/package.json
RUN npm install --no-audit --no-fund
COPY vitest-jest/ /sample/
CMD ["node", "--test", "flakeradar-quarantine.test.mjs"]
