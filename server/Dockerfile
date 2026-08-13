FROM node:24-slim

WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --omit=dev --ignore-scripts
COPY src ./src

RUN mkdir -p /data
ENV NODE_ENV=production
EXPOSE 8787

CMD ["node", "src/postgres-server.js"]
