import bundle from './direct-bundle.cjs';
const args=process.argv.slice(2), values=new Map();
for(let i=0;i<args.length;i+=2) {
  if(!['--input','--output','--codesign-identity'].includes(args[i]) || !args[i+1] || values.has(args[i])) throw new Error('Invalid AstreaAI signing arguments');
  values.set(args[i],args[i+1]);
}
for(const flag of ['--input','--output','--codesign-identity']) if(!values.has(flag)) throw new Error('Missing AstreaAI signing argument');
await bundle.signMacBundles(values.get('--input'),values.get('--output'),values.get('--codesign-identity'),process.env.APPLE_TEAM_ID);
