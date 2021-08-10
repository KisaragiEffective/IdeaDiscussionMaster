use serenity::{
    async_trait,
    model::{
        channel::{Reaction, ReactionType},
        gateway::Ready,
        id::ChannelId,
    },
    prelude::{Context, EventHandler},
};
use std::{convert::From, sync::atomic::Ordering};

use crate::{
    domains::{discord_embed, discussion, redmine},
    globals::{
        agendas::{self, AgendaStatus},
        current_agenda_id, record_id, voice_chat_channel_id, voted_message_id,
    },
};

pub struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        println!("Connected as {}", ready.user.name);
    }

    async fn reaction_add(&self, ctx: Context, reaction: Reaction) {
        let voted_message_id = {
            let cached_voted_message_id = {
                let data_read = ctx.data.read().await;
                data_read
                    .get::<voted_message_id::VotedMessageId>()
                    .expect("Expected VotedMessageId in TypeMap.")
                    .clone()
            };
            cached_voted_message_id.load(Ordering::Relaxed)
        };
        if let Ok(user) = reaction.user(&ctx.http).await {
            if user.bot {
                return;
            }
        }
        if voted_message_id == 0 {
            return;
        }

        // TODO: だだかぶりなのでまとめる→VoiceStatesを返す関数に
        let vc_id = {
            let cached_voice_chat_channel_id = {
                let data_read = ctx.data.read().await;
                data_read
                    .get::<voice_chat_channel_id::VoiceChatChannelId>()
                    .expect("Expected VoiceChatChannelId in TypeMap.")
                    .clone()
            };
            cached_voice_chat_channel_id.load(Ordering::Relaxed)
        };
        let guild_id = if let Some(id) = reaction.guild_id {
            id
        } else {
            println!("投票メッセージに対してリアクションが行われましたが、guild_idが見つかりませんでした。");

            return;
        };
        let guild = if let Some(guild) = ctx.cache.guild(guild_id).await {
            guild
        } else {
            println!(
                    "投票メッセージに対してリアクションが行われましたが、guildが見つかりませんでした。（guild_id: {}）",
                    guild_id
                );
            return;
        };
        let vc_members = guild
            .voice_states
            .iter()
            .filter(|(_, state)| state.channel_id.unwrap() == ChannelId(vc_id))
            .count();
        let half_of_vc_members = vc_members / 2;

        let choices = vec![AgendaStatus::Approved, AgendaStatus::Declined];
        let status_reaction = if let Some(emoji) = choices
            .iter()
            .find(|status| reaction.emoji.unicode_eq(&status.emoji().to_string()))
        {
            emoji.to_owned()
        } else {
            return;
        };
        let reaction_counts = if let Ok(num) = ctx
            .http
            .as_ref()
            .get_reaction_users(
                reaction.channel_id.as_u64().to_owned(),
                reaction.message_id.as_u64().to_owned(),
                &ReactionType::from(status_reaction.emoji()),
                100,
                None,
            )
            .await
        {
            num.len() - 1
        } else {
            return;
        };
        // TODO: 過半数を超えていたら以下の操作をする
        // redmineのステータス変更

        if reaction_counts <= half_of_vc_members {
            return;
        }

        {
            let cached_voted_message_id = {
                let data_read = ctx.data.read().await;
                data_read
                    .get::<voted_message_id::VotedMessageId>()
                    .expect("Expected VotedMessageId in TypeMap.")
                    .clone()
            };
            cached_voted_message_id.store(0, Ordering::Relaxed);
        }

        let record_id = {
            let cached_record_id = {
                let data_read = ctx.data.read().await;
                data_read
                    .get::<record_id::RecordId>()
                    .expect("Expected RecordId in TypeMap.")
                    .clone()
            };
            cached_record_id.load(Ordering::Relaxed)
        };
        let current_agenda_id = {
            let cached_current_agenda_id = {
                let data_read = ctx.data.read().await;
                data_read
                    .get::<current_agenda_id::CurrentAgendaId>()
                    .expect("Expected CurrentAgendaId in TypeMap.")
                    .clone()
            };
            cached_current_agenda_id.load(Ordering::Relaxed)
        };

        let _ = reaction
            .channel_id
            .send_message(&ctx.http, |msg| {
                msg.embed(|embed| {
                    match status_reaction {
                        AgendaStatus::Approved => {
                            discord_embed::default_success_embed(embed, record_id)
                        }
                        AgendaStatus::Declined => {
                            discord_embed::default_failure_embed(embed, record_id)
                        }
                        _ => embed,
                    }
                    .title(format!(
                        "採決終了: #{}は{}されました",
                        current_agenda_id,
                        status_reaction.ja()
                    ))
                })
            })
            .await;

        agendas::write(&ctx, current_agenda_id, status_reaction).await;

        let next_agenda_id = discussion::go_to_next_agenda(&ctx).await;
        // TODO: まとめる
        let _ = reaction
            .channel_id
            .send_message(&ctx.http, |msg| {
                msg.embed(|embed| match next_agenda_id {
                    // TODO: 議題のタイトルと説明を追加
                    Some(id) => discord_embed::default_success_embed(embed, record_id)
                        .title(format!("次の議題は#{}です", id))
                        .field(
                            "議題チケット",
                            format!("{}{}", redmine::REDMINE_ISSUE_URL, id),
                            false,
                        ),
                    None => discord_embed::default_failure_embed(embed, record_id)
                        .title("次の議題はありません")
                        .description("Redmine上で提起されていた議題は全て処理されました。"),
                })
            })
            .await;
    }
}
