FROM node:24-slim

WORKDIR /app
COPY package.json ./
COPY src ./src

RUN mkdir -p /data
ENV NODE_ENV=production
EXPOSE 8787

CMD ["node", "src/server.js"]
