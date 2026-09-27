async function main() {
  const app = await createPostgresServer();
  const port = Number(process.env.SOLUM_PORT || 8787);
  app.server.listen(port, '0.0.0.0', () => console.log(`Solum cloud listening on ${port} (PostgreSQL)`));
  const shutdown = async () => {
    await app.close();
    process.exit(0);
  };
  process.once('SIGTERM', shutdown);
  process.once('SIGINT', shutdown);
}

if (require.main === module) {
  main().catch((error) => {
    console.error('startup failed', safeErrorLog(error));
    process.exit(1);
  });
}

module.exports = {
  createPostgresServer, verifyAccessToken, withTenant, parseModelCatalog, usageProjection,
  validateCompletionOptions, readBodyLimited, recoverTenantStaleAiRequests, proxyCompletion,
  optionalBoundedNumber, optionalHttpUrl, requiredOpaqueString, safeDatabaseInteger,
  parseBackupPlan, parseTakeover, parseWriterClaim, parseIfMatch, buildConfig,
};
