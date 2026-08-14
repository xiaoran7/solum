FROM node:24-slim

WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --omit=dev --ignore-scripts
COPY src ./src
COPY scripts ./scripts

RUN mkdir -p /data && chown node:node /data
ENV NODE_ENV=production
EXPOSE 8787

USER node

CMD ["node", "src/postgres-server.js"]
