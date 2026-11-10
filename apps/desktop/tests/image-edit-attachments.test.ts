import assert from "node:assert/strict";
import test from "node:test";
import {acceptsAttachment,attachmentAccept} from "@axiom/chat-core/attachments";
import type {ProviderModel} from "@axiom/chat-core/types";
import type {PromptAttachment} from "@axiom/chat-core/viewTypes";

const textOnly={id:"text-only",supportsImages:false,fileMimeTypes:[]} as unknown as ProviderModel;
const image:PromptAttachment={kind:"image",name:"photo.png",image:{mimeType:"image/png",data:"iVBORw0KGgo="}};
const pdf:PromptAttachment={kind:"file",name:"private.pdf",file:{name:"private.pdf",mimeType:"application/pdf",data:"JVBERg=="}};

test("text-only models cannot upload image references or gain unrelated file support",()=>{
 assert.equal(acceptsAttachment(textOnly,image),false);
 assert.equal(acceptsAttachment(textOnly,pdf),false);
 assert.equal(acceptsAttachment(undefined,image),false);
 assert.equal(attachmentAccept(textOnly),".unsupported");
 assert.equal(textOnly.supportsImages,false);
});

test("vision image uploads are always available while documents retain their own capability",()=>{
 const vision={...textOnly,supportsImages:true};
 assert.equal(acceptsAttachment(vision,image),true);
 assert.equal(acceptsAttachment(vision,pdf),false);
 assert.match(attachmentAccept(vision),/image\/jpeg/);
 assert.doesNotMatch(attachmentAccept(vision),/application\/pdf/);
 const documents={...textOnly,fileMimeTypes:["application/pdf"]};
 assert.equal(acceptsAttachment(documents,pdf),true);
 assert.equal(acceptsAttachment(documents,image),false);
});
