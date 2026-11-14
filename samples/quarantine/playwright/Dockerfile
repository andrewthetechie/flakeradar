FROM node:24-slim
# The Playwright quarantine sample. Build with the samples/quarantine/
# directory as the context (samples/quarantine/test.sh does this): every COPY
# path is prefixed with playwright/.
WORKDIR /sample
# The dependency layer comes first so an edit to the sample's own files does not
# reinstall: example/ resolves @playwright/test from /sample/node_modules. Node
# 24 strips the helper's TypeScript types, so the .mjs test and the config can
# import flakeradar-quarantine.ts with no build step.
COPY playwright/package.json /sample/package.json
RUN npm install --no-audit --no-fund
COPY playwright/ /sample/
CMD ["node", "--test", "flakeradar-quarantine.test.mjs"]
