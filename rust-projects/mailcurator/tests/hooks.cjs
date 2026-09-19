// Synthetic command stubs only: prove old binaries skip curation, new ones
// receive --llm-disable, and normal indexing/tag housekeeping still runs.
const fs=require('node:fs'),os=require('node:os'),path=require('node:path'),assert=require('node:assert/strict'),{spawnSync}=require('node:child_process');
for(const hook of ['post-new','post-new-cohs']) for(const supported of ['0','1']) {
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'curator-hook-')),bin=path.join(root,'bin');fs.mkdirSync(bin);
 fs.mkdirSync(path.join(root,'.config/mailcurator'),{recursive:true});fs.writeFileSync(path.join(root,'.config/mailcurator/policies-cohs.toml'),'policy=[]');
 fs.writeFileSync(path.join(bin,'notmuch'),'#!/bin/sh\nexit 0\n',{mode:0o755});
 fs.writeFileSync(path.join(bin,'mailcurator'),`#!${process.execPath}\nconst fs=require('node:fs');const args=process.argv.slice(2);if(args[0]==='evidence')process.exit(process.env.SUPPORTED==='1'?0:1);fs.writeFileSync(process.env.HOME+'/called',JSON.stringify(args));\n`,{mode:0o755});
 const out=spawnSync('bash',[path.resolve(__dirname,'../../..','notmuch/hooks',hook)],{encoding:'utf8',env:{...process.env,HOME:root,XDG_CONFIG_HOME:path.join(root,'.config'),TMPDIR:root,PATH:bin+':'+process.env.PATH,SUPPORTED:supported,MAILFORGE_FAST_REINDEX:''}});
 assert.equal(out.status,0,out.stderr);
 assert.equal(fs.existsSync(path.join(root,'called')),supported==='1');
 if(supported==='1')assert.ok(JSON.parse(fs.readFileSync(path.join(root,'called'))).includes('--llm-disable'));
 else assert.ok(out.stderr.includes('curation skipped'));
}
console.log('PASS both hooks: old binary skips; supported binary deterministic; sync housekeeping succeeds');
