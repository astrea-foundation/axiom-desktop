import {mkdir,readFile,writeFile} from 'node:fs/promises';
import path from 'node:path';
import {requireCompleteRelease} from '../packages/desktop-releases/manifest.mjs';
import {verifyRelease} from '../packages/desktop-releases/signing.mjs';
import {verifySetup} from '../packages/desktop-releases/windows-setup.mjs';
import {createHash} from 'node:crypto';

const [directory,setupDirectory,output] = process.argv.slice(2);
if(!directory || !setupDirectory || !output)throw new Error('Usage: prepare-store-submission.mjs RELEASE_DIRECTORY SETUP_DIRECTORY OUTPUT_DIRECTORY');
const release=requireCompleteRelease(JSON.parse(await readFile(path.join(directory,'manifest.json'),'utf8')));
const keys=process.env.AXIOM_UPDATE_PUBLIC_KEYS ?? '';
if(!keys.split(',').some(key=>{try{verifyRelease(release,key.trim());return true;}catch{return false;}}))throw new Error('Store preparation requires a trusted release');
const setup=verifySetup(JSON.parse(await readFile(path.join(setupDirectory,'windows-setup.json'),'utf8')),keys);
if(setup.releaseVersion!==release.version || setup.revision!==release.revision)throw new Error('Setup belongs to a different release');
const installer=release.downloads.find(file=>file.platform==='win' && file.arch==='universal' && file.product==='desktop' && file.format==='exe');
if(!installer)throw new Error('Store requires the full offline Desktop installer');
const bytes=await readFile(path.join(directory,installer.name));
if(bytes.length!==installer.bytes || createHash('sha256').update(bytes).digest('hex')!==installer.sha256)throw new Error('Store installer changed after signing');
const candidate={schemaVersion:1,status:'prepared-awaiting-qualification-and-certification',version:release.version,revision:release.revision,installer,
  submission:{packageType:'exe',installerUrl:installer.url,silentInstallParameters:'/S',silentUpdateParameters:'/S',silentUninstallParameters:'/S',supportedArchitectures:['x64','arm64'],expectedPublisher:setup.publisher,productId:null,privacyPolicyUrl:'https://axiom.stream/privacy',websiteUrl:'https://axiom.stream',supportUrl:null},
  firstLaunchUpdate:{automaticDownloadAndInstall:true,requiresSignIn:false,offlineFailureAllowsContinue:true},
  qualification:{releaseRun:process.env.GITHUB_RUN_ID?`https://github.com/${process.env.GITHUB_REPOSITORY}/actions/runs/${process.env.GITHUB_RUN_ID}`:null,complete:false,
    remaining:['Clean standard-user default-path silent offline installation on x64 and ARM64','First-launch latest/no-update/offline/error/cancel/restart acceptance on signed packages','Defender scan, upgrades and uninstall acceptance','Partner Center listing, product identity, privacy review and reviewer account','Microsoft certification']}};
await mkdir(output,{recursive:true});
await writeFile(path.join(output,'candidate.json'),JSON.stringify(candidate,null,2)+'\n');
await writeFile(path.join(output,'certification-notes.txt'),`Axiom ${release.version} is a complete offline EXE installation for Windows x64 and ARM64. Submit ${installer.url} with silent installation and update arguments /S. Installation does not launch the app. Internet is required for sign-in and model usage, not for installing this package.\n\nOn a fresh installation, the first user-initiated app launch checks Axiom's signed stable feed. If a newer version exists, the app downloads, verifies, installs and restarts. If the check fails, the user can retry or use the complete installed version. Existing users retain the normal update action.\n\nAxiom is free to install. Model usage requires an account and prepaid credit. Provide a dedicated funded test account and sign-in instructions in Partner Center's private reviewer fields. Finish the qualification items in candidate.json before submission. Do not submit AxiomSetup: it downloads app binaries and is for the website only.\n`);
console.log(`Prepared Store handoff for the full offline Axiom ${release.version} installer; certification remains required.`);
