// Generate the Android and iOS icon sets from the canonical Solum SVG mark.
//
// Tauri handles platform-specific resizing and adaptive-icon safe zones. The
// canonical SVG owns the white background and visual centering, so both mobile
// platforms render the same artwork.
//
// Usage: node scripts/gen-icon.mjs
import { spawnSync } from 'node:child_process';
import {
  copyFileSync,
  cpSync,
  existsSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const source = join(root, 'crates/solum-app/icons/solum-mark-final.svg');
const iosDir = join(root, 'crates/solum-app/icons/ios');
const androidRes = join(root, 'crates/solum-app/gen/android/app/src/main/res');
const staging = mkdtempSync(join(tmpdir(), 'solum-mobile-icons-'));

try {
  const result = spawnSync(
    process.platform === 'win32' ? 'cargo.exe' : 'cargo',
    ['tauri', 'icon', source, '--output', staging, '--ios-color', '#fff'],
    { cwd: root, encoding: 'utf8' },
  );
  if (result.status !== 0) {
    process.stderr.write(result.stdout ?? '');
    process.stderr.write(result.stderr ?? '');
    process.exit(result.status ?? 1);
  }

  for (const name of readdirSync(join(staging, 'ios'))) {
    if (name.endsWith('.png')) {
      copyFileSync(join(staging, 'ios', name), join(iosDir, name));
    }
  }

  const generatedAndroid = join(staging, 'android');
  for (const name of ['mipmap-mdpi', 'mipmap-hdpi', 'mipmap-xhdpi', 'mipmap-xxhdpi', 'mipmap-xxxhdpi']) {
    cpSync(join(generatedAndroid, name), join(androidRes, name), {
      recursive: true,
      force: true,
    });
  }
  const backgroundSource = join(generatedAndroid, 'values/ic_launcher_background.xml');
  const backgroundTarget = join(androidRes, 'values/ic_launcher_background.xml');
  writeFileSync(backgroundTarget, `${readFileSync(backgroundSource, 'utf8').trimEnd()}\n`);

  const required = [
    join(iosDir, 'AppIcon-512@2x.png'),
    join(androidRes, 'mipmap-xxxhdpi/ic_launcher.png'),
    join(androidRes, 'mipmap-xxxhdpi/ic_launcher_foreground.png'),
    join(androidRes, 'values/ic_launcher_background.xml'),
  ];
  if (required.some((path) => !existsSync(path))) {
    throw new Error('mobile icon generation did not produce every required resource');
  }

  console.log(`generated ${readdirSync(join(staging, 'ios')).length} iOS icons`);
  console.log('generated Android legacy, round, and adaptive icon resources');
} finally {
  rmSync(staging, { recursive: true, force: true });
}
