    if (req.method === 'POST' && url.pathname === '/v1/push') {
      const tenantId = await authenticate(req);
      const device = requiredString(req.headers['x-device'], 'invalid_device', 128);
      const blob = await readBytes(req, MAX_BLOB_BYTES);
      if (blob.length === 0) throw new HttpError(400, 'bad_blob');
      const seq = await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        await touchActiveDevice(client, tenantId, device, config.tenantMaxDevices);
        await ensureBlobQuota(client, tenantId, blob.length, config);
        const inserted = await client.query(
          'insert into sync.blobs(tenant_id,device_id,ciphertext) values($1,$2,$3) returning seq',
          [tenantId, device, blob]
        );
        return safeDatabaseInteger(inserted.rows[0].seq, 'blob_seq');
      });
      return json(res, 200, { seq });
    }

    if (req.method === 'GET' && url.pathname === '/v1/pull') {
      const tenantId = await authenticate(req);
      const since = parseNonNegativeInt(url.searchParams.get('since'), 0);
      const device = requiredString(url.searchParams.get('device'), 'invalid_device', 128);
      const result = await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        await touchActiveDevice(client, tenantId, device, config.tenantMaxDevices);
        // Window functions run before LIMIT, so sizing ciphertexts inside the
        // same query level would detoast every blob beyond `since` (up to the
        // tenant's whole history). Pick candidates by size first, then fetch
        // ciphertext only for the rows that survive the byte cap.
        const pulled = await client.query(
          `with candidates as (
             select seq,device_id,octet_length(ciphertext) as blob_bytes
             from sync.blobs
             where tenant_id=$1 and seq>$2 and device_id<>$3
             order by seq
             limit $4
           ), selected as (
             select seq,device_id,
                    sum(blob_bytes) over (order by seq) as cumulative_bytes,
                    row_number() over (order by seq) as row_num
             from candidates
           )
           select b.seq,b.device_id,b.ciphertext
           from selected s join sync.blobs b on b.seq=s.seq
           where s.cumulative_bytes<=$5 or s.row_num=1
           order by b.seq`,
          [tenantId, since, device, MAX_PULL_ROWS, MAX_PULL_BYTES]
        );
        const oldest = await client.query(
          'select coalesce(min(seq),0) as oldest_seq from sync.blobs where tenant_id=$1', [tenantId]
        );
        return {
          oldest_seq: safeDatabaseInteger(oldest.rows[0].oldest_seq, 'oldest_blob_seq'),
          blobs: pulled.rows.map((row) => ({
            seq: safeDatabaseInteger(row.seq, 'blob_seq'),
            device: row.device_id, blob: row.ciphertext.toString('base64'),
          })),
        };
      });
      return json(res, 200, result);
    }

    if (req.method === 'GET' && url.pathname === '/v1/stats') {
      const tenantId = await authenticate(req);
      const stats = await withTenant(pool, tenantId, async (client) => {
        const totals = await client.query(
          `select count(*) as total_blobs,coalesce(sum(octet_length(ciphertext)),0) as total_bytes,
                  coalesce(min(seq),0) as oldest_seq,coalesce(max(seq),0) as newest_seq
           from sync.blobs where tenant_id=$1`, [tenantId]
        );
        const devices = await client.query(
          `select device_id,count(*) as blob_count,coalesce(sum(octet_length(ciphertext)),0) as bytes,
                  max(seq) as last_seq,max(received_at) as last_received_at
           from sync.blobs where tenant_id=$1 group by device_id order by max(seq) desc`, [tenantId]
        );
        const row = totals.rows[0];
        return {
          total_blobs: safeDatabaseInteger(row.total_blobs, 'total_blobs'),
          total_bytes: safeDatabaseInteger(row.total_bytes, 'total_bytes'),
          oldest_seq: safeDatabaseInteger(row.oldest_seq, 'oldest_blob_seq'),
          newest_seq: safeDatabaseInteger(row.newest_seq, 'newest_blob_seq'),
          devices: devices.rows.map((item) => ({
            device: item.device_id,
            blob_count: safeDatabaseInteger(item.blob_count, 'device_blob_count'),
            bytes: safeDatabaseInteger(item.bytes, 'device_blob_bytes'),
            last_seq: safeDatabaseInteger(item.last_seq, 'device_last_seq'),
            last_received_at: item.last_received_at,
          })),
          tenant: tenantId, auth_mode: 'account',
        };
      });
      return json(res, 200, stats);
    }

    if (req.method === 'GET' && url.pathname === '/v1/devices') {
      const tenantId = await authenticate(req);
      const devices = await withTenant(pool, tenantId, async (client) => {
        const result = await client.query(
          `select device_id,protocol_version,last_seen_at,revoked_at
           from sync.devices where tenant_id=$1 order by last_seen_at desc`, [tenantId]
        );
        return result.rows;
      });
      return json(res, 200, { devices });
    }

