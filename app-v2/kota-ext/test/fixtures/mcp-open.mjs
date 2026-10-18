/** Test-only browser stand-in: visits one loopback fake authorization URL. */
const target=new URL(process.argv[2]);
if(target.protocol!=='http:'||target.hostname!=='127.0.0.1')throw Error('Non-loopback test browser target');
await fetch(target);
